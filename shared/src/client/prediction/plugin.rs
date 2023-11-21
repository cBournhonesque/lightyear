use std::marker::PhantomData;

use bevy::prelude::{
    apply_deferred, App, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PreUpdate,
    Res, SystemSet,
};

use crate::client::prediction::despawn::{
    remove_component_for_despawn_predicted, remove_despawn_marker,
};
use crate::plugin::sets::{FixedUpdateSet, MainSet};
use crate::{ComponentProtocol, Protocol};

use super::predicted_history::{add_component_history, update_component_history};
use super::rollback::{client_rollback_check, increment_rollback_tick, run_rollback};
use super::{
    spawn_predicted_entity, PredictedComponent, PredictedComponentMode, Rollback, RollbackState,
};

pub struct PredictionPlugin<P: Protocol> {
    always_rollback: bool,
    // rollback_tick_stragegy:
    // - either we consider that the server sent the entire world state at last_received_server_tick
    // - or not, and we have to check the oldest tick across all components that don't match
    _marker: PhantomData<P>,
}

impl<P: Protocol> Default for PredictionPlugin<P> {
    fn default() -> Self {
        Self {
            always_rollback: false,
            _marker: PhantomData::default(),
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
    // TODO: a bit confusing, maybe only rollback should do that?
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

pub fn should_rollback<C: PredictedComponent>() -> bool {
    matches!(C::mode(), PredictedComponentMode::Rollback)
}

/// Returns true if we are doing rollback
pub fn is_in_rollback(rollback: Res<Rollback>) -> bool {
    match rollback.state {
        RollbackState::ShouldRollback { .. } => true,
        _ => false,
    }
}

// We want to run prediction:
// - after we received network events (PreUpdate)
// - before we run physics FixedUpdate (to not have to redo-them)

// - a PROBLEM is that ideally we would like to rollback the physics simulation
//   up to the client tick before we just updated the time. Maybe that's not a problem.. but we do need to keep track of the ticks correctly
//  the tick we rollback to would not be the current client tick ?

pub fn add_prediction_systems<C: PredictedComponent, P: Protocol>(app: &mut App) {
    // TODO: maybe create an overarching prediction set that contains all others?
    app.add_systems(
        PreUpdate,
        (add_component_history::<C, P>).in_set(PredictionSet::SpawnHistory),
    );
    app.add_systems(
        PreUpdate,
        (client_rollback_check::<C, P>.run_if(should_rollback::<C>))
            .in_set(PredictionSet::CheckRollback),
    );
    app.add_systems(
        FixedUpdate,
        (
            // we need to run this during fixed update to know accurately the history for each tick
            update_component_history::<C, P>
                .run_if(should_rollback::<C>)
                .in_set(PredictionSet::UpdateHistory)
        ),
    );
    app.add_systems(
        FixedUpdate,
        remove_component_for_despawn_predicted::<C>.in_set(PredictionSet::EntityDespawn),
    );
}

impl<P: Protocol> Plugin for PredictionPlugin<P> {
    fn build(&self, app: &mut App) {
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
                MainSet::Receive,
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
            (
                apply_deferred.in_set(FixedUpdateSet::MainFlush),
                apply_deferred.in_set(PredictionSet::EntityDespawnFlush),
            ),
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
            ((
                FixedUpdateSet::Main,
                FixedUpdateSet::MainFlush,
                PredictionSet::EntityDespawn,
                PredictionSet::EntityDespawnFlush,
                PredictionSet::UpdateHistory,
                PredictionSet::IncrementRollbackTick.run_if(is_in_rollback),
            )
                .chain()),
        );
        // TODO: add apply user inputs? maybe that should be done by the user in their own physics systems!
        //  apply user input should use client_tick normally, but use rollback tick during rollback!
        app.add_systems(
            FixedUpdate,
            (increment_rollback_tick.in_set(PredictionSet::IncrementRollbackTick)),
        );
        app.add_systems(
            FixedUpdate,
            remove_despawn_marker.in_set(PredictionSet::EntityDespawn),
        );
    }
}
