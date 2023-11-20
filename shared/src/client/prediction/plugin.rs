use std::marker::PhantomData;

use bevy::prelude::{
    apply_deferred, App, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PreUpdate,
    SystemSet,
};

use crate::plugin::sets::{FixedUpdateSet, MainSet};
use crate::replication::prediction::is_in_rollback;
use crate::{ComponentProtocol, Protocol};

use super::predicted_history::{add_component_history, update_component_history};
use super::rollback::{client_rollback_check, increment_rollback_tick, run_rollback};
use super::{spawn_predicted_entity, PredictedComponent, Rollback, RollbackState};

pub struct PredictionPlugin<P: Protocol> {
    // always_rollback: bool
    // rollback_tick_stragegy:
    // - either we consider that the server sent the entire world state at last_received_server_tick
    // - or not, and we have to check the oldest tick across all components that don't match
    _marker: PhantomData<P>,
}

impl<P: Protocol> Default for PredictionPlugin<P> {
    fn default() -> Self {
        Self {
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
    /// Add component history for all predicted entities' predicted components
    SpawnHistory,
    /// Check if rollback is needed, potentially clear history and snap prediction histories to server state
    CheckRollback,
    /// Perform rollback
    Rollback,
    // FixedUpdate Sets
    /// Increment the rollback tick after the main fixed-update physics loop has run
    IncrementRollbackTick,
    /// Update the client's predicted history; runs after each physics step in the FixedUpdate Schedule
    UpdateHistory,
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
        (client_rollback_check::<C, P>).in_set(PredictionSet::CheckRollback),
    );
    app.add_systems(
        FixedUpdate,
        (
            // we need to run this during fixed update to know accurately the history for each tick
            update_component_history::<C, P>.in_set(PredictionSet::UpdateHistory)
        ),
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
                PredictionSet::SpawnHistory,
                PredictionSet::CheckRollback,
                PredictionSet::Rollback.run_if(is_in_rollback),
            )
                .chain(),
            // (
            //     PredictionSet::SpawnPrediction.after(MainSet::Receive),
            //     PredictionSet::SpawnHistory.after(PredictionSet::SpawnPrediction),
            //     PredictionSet::CheckRollback.after(PredictionSet::SpawnHistory),
            //     PredictionSet::Rollback
            //         .after(PredictionSet::CheckRollback)
            //         .run_if(is_in_rollback),
            // ),
        );
        app.add_systems(
            PreUpdate,
            // apply_deferred because we spawn a predicted entity
            ((spawn_predicted_entity, apply_deferred).chain())
                .in_set(PredictionSet::SpawnPrediction),
        );
        // 2. (in prediction_systems) add ComponentHistory and a apply_deferred after
        app.add_systems(
            PreUpdate,
            // apply deferred because we spawn the PredictedComponent and ComponentHistory
            (apply_deferred)
                .after(PredictionSet::SpawnHistory)
                .before(PredictionSet::CheckRollback),
        );
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
                PredictionSet::UpdateHistory,
                PredictionSet::IncrementRollbackTick.run_if(is_in_rollback),
            )
                .chain()), // (
                           //     PredictionSet::UpdateHistory
                           //         .after(FixedUpdateSet::Main),
                           //     PredictionSet::IncrementRollbackTick
                           //         .after(PredictionSet::UpdateHistory)
                           //         .run_if(is_in_rollback),
                           // ),
        );
        // TODO: add apply user inputs? maybe that should be done by the user in their own physics systems!
        //  apply user input should use client_tick normally, but use rollback tick during rollback!
        app.add_systems(
            FixedUpdate,
            (increment_rollback_tick.in_set(PredictionSet::IncrementRollbackTick)),
        );
        // add an apply_deferred to apply any potential entity actions
        app.add_systems(
            FixedUpdate,
            (apply_deferred)
                .after(FixedUpdateSet::Main)
                .before(PredictionSet::UpdateHistory),
        );
    }
}
