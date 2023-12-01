use std::marker::PhantomData;

use bevy::prelude::{
    apply_deferred, App, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PreUpdate,
    Res, SystemSet,
};

use crate::client::components::SyncComponent;
use crate::client::prediction::despawn::{
    remove_component_for_despawn_predicted, remove_despawn_marker,
};
use crate::client::prediction::predicted_history::update_prediction_history;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::Protocol;
use crate::shared::sets::{FixedUpdateSet, MainSet};

use super::predicted_history::{add_component_history, apply_confirmed_update};
use super::rollback::{client_rollback_check, increment_rollback_tick, run_rollback};
use super::{spawn_predicted_entity, ComponentSyncMode, Rollback, RollbackState};

#[derive(Debug, Clone, Copy, Default)]
pub struct PredictionConfig {
    /// If true, we completely disable the prediction plugin
    disable: bool,
    always_rollback: bool,
}

impl PredictionConfig {
    pub fn disable(mut self, disable: bool) -> Self {
        self.disable = disable;
        self
    }

    pub fn always_rollback(mut self, always_rollback: bool) -> Self {
        self.always_rollback = always_rollback;
        self
    }
}

pub struct PredictionPlugin<P: Protocol> {
    config: PredictionConfig,
    // rollback_tick_stragegy:
    // - either we consider that the server sent the entire world state at last_received_server_tick
    // - or not, and we have to check the oldest tick across all components that don't match
    _marker: PhantomData<P>,
}

impl<P: Protocol> PredictionPlugin<P> {
    pub(crate) fn new(config: PredictionConfig) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }
}

impl<P: Protocol> Default for PredictionPlugin<P> {
    fn default() -> Self {
        Self {
            config: PredictionConfig::default(),
            _marker: PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PredictionSet {
    // PreUpdate Sets
    // // Contains the other pre-update prediction stes
    // PreUpdatePrediction,
    /// Spawn predicted entities,
    SpawnPrediction,
    SpawnPredictionFlush,
    /// Add component history for all predicted entities' predicted components
    SpawnHistory,
    SpawnHistoryFlush,
    /// Check if rollback is needed, potentially clear history and snap prediction histories to server state
    CheckRollback,
    // we might need a flush because check-rollback might remove/add components.
    // TODO: a bit confusing that check rollback needs a flush. It's because check-rollback applies the initial rollback state, maybe only rollback should do that?
    CheckRollbackFlush,
    /// Perform rollback
    Rollback,
    // NOTE: no need to add RollbackFlush because running a schedule (which we do for rollback) will flush all commands at the end of each run
    // FixedUpdate Sets
    /// Increment the rollback tick after the main fixed-update physics loop has run
    IncrementRollbackTick,
    /// Set to deal with predicted/confirmed entities getting despawned
    EntityDespawn,
    EntityDespawnFlush,
    /// Update the client's predicted history; runs after each physics step in the FixedUpdate Schedule
    UpdateHistory,
}

pub fn should_rollback<C: SyncComponent>() -> bool {
    matches!(C::mode(), ComponentSyncMode::Full)
}

/// Returns true if we are doing rollback
pub fn is_in_rollback(rollback: Res<Rollback>) -> bool {
    matches!(rollback.state, RollbackState::ShouldRollback { .. })
}

// We want to run prediction:
// - after we received network events (PreUpdate)
// - before we run physics FixedUpdate (to not have to redo-them)

// - a PROBLEM is that ideally we would like to rollback the physics simulation
//   up to the client tick before we just updated the time. Maybe that's not a problem.. but we do need to keep track of the ticks correctly
//  the tick we rollback to would not be the current client tick ?

pub fn add_prediction_systems<C: SyncComponent, P: Protocol>(app: &mut App) {
    // TODO: maybe create an overarching prediction set that contains all others?
    app.add_systems(
        PreUpdate,
        (
            (add_component_history::<C, P>).in_set(PredictionSet::SpawnHistory),
            // for SyncMode::Simple, just copy the confirmed components
            (apply_confirmed_update::<C, P>).in_set(PredictionSet::CheckRollback),
            // for SyncMode::Full, we need to check if we need to rollback
            (client_rollback_check::<C, P>.run_if(should_rollback::<C>))
                .in_set(PredictionSet::CheckRollback),
        ),
    );
    app.add_systems(
        FixedUpdate,
        // we need to run this during fixed update to know accurately the history for each tick
        update_prediction_history::<C, P>
            .run_if(should_rollback::<C>)
            .in_set(PredictionSet::UpdateHistory),
    );
    app.add_systems(
        FixedUpdate,
        remove_component_for_despawn_predicted::<C>.in_set(PredictionSet::EntityDespawn),
    );
}

impl<P: Protocol> Plugin for PredictionPlugin<P> {
    fn build(&self, app: &mut App) {
        if self.config.disable {
            return;
        }
        P::Components::add_prediction_systems(app);

        // RESOURCES
        app.insert_resource(Rollback {
            state: RollbackState::Default,
        });

        // PreUpdate systems:
        // 1. Receive confirmed entities, add Confirmed and Predicted components
        app.configure_sets(
            PreUpdate,
            (
                MainSet::ReceiveFlush,
                PredictionSet::SpawnPrediction,
                PredictionSet::SpawnPredictionFlush,
                PredictionSet::SpawnHistory,
                PredictionSet::SpawnHistoryFlush,
                PredictionSet::CheckRollback,
                PredictionSet::CheckRollbackFlush,
                PredictionSet::Rollback.run_if(is_in_rollback),
            )
                .chain(),
        );
        app.add_systems(
            PreUpdate,
            (
                // TODO: we want to run this flushes only if something actually happened in the previous set!
                //  because running the flush-system is expensive (needs exclusive world access)
                //  check how I can do this in bevy
                apply_deferred.in_set(PredictionSet::SpawnPredictionFlush),
                apply_deferred.in_set(PredictionSet::SpawnHistoryFlush),
                apply_deferred.in_set(PredictionSet::CheckRollbackFlush),
            ),
        );
        app.add_systems(
            FixedUpdate,
            (apply_deferred.in_set(PredictionSet::EntityDespawnFlush),),
        );

        app.add_systems(
            PreUpdate,
            spawn_predicted_entity.in_set(PredictionSet::SpawnPrediction),
        );
        // 2. (in prediction_systems) add ComponentHistory and a apply_deferred after
        // 3. (in prediction_systems) Check if we should do rollback, clear histories and snap prediction's history to server-state
        // 4. Potentially do rollback
        app.add_systems(
            PreUpdate,
            (run_rollback::<P>).in_set(PredictionSet::Rollback),
        );

        // FixedUpdate systems
        // 1. Update client tick (don't run in rollback)
        // 2. Run main physics/game fixed-update loop
        // 3. Increment rollback tick (only run in fallback)
        // 4. Update predicted history
        app.configure_sets(
            FixedUpdate,
            (
                FixedUpdateSet::Main,
                FixedUpdateSet::MainFlush,
                PredictionSet::EntityDespawn,
                PredictionSet::EntityDespawnFlush,
                PredictionSet::UpdateHistory,
                PredictionSet::IncrementRollbackTick.run_if(is_in_rollback),
            )
                .chain(),
        );
        app.add_systems(
            FixedUpdate,
            increment_rollback_tick.in_set(PredictionSet::IncrementRollbackTick),
        );
        app.add_systems(
            FixedUpdate,
            remove_despawn_marker.in_set(PredictionSet::EntityDespawn),
        );
    }
}
