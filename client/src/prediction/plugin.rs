use crate::prediction::predicted_history::update_component_history;
use crate::prediction::systems::{client_rollback_check, run_rollback};
use bevy::prelude::{App, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PreUpdate};

pub struct PredictionPlugin;

// We want to run prediction:
// - after we received network events (PreUpdate)
// - before we run physics FixedUpdate (to not have to redo-them)

// - a PROBLEM is that ideally we would like to rollback the physics simulation
//   up to the client tick before we just updated the time. Maybe that's not a problem.. but we do need to keep track of the ticks correctly
//  the tick we rollback to would not be the current client tick ?

impl Plugin for PredictionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            // ((
            // by this point, the client ticks haven't been updated yet, so we can just rollback between the last server tick and the current client tick
            client_rollback_check,
            // run_rollback,
            // )
            //     .chain()),
        );
        app.add_systems(
            FixedUpdate,
            (
                // we need to run this during fixed update to know accurately the history for each tick
                update_component_history
            ),
        );
    }
}
