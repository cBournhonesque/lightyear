//! Handles client-side prediction
use std::fmt::Debug;

use bevy::prelude::{Added, Commands, Component, Entity, Query, ResMut, Resource};
use tracing::info;

pub use despawn::{PredictionCommandsExt, PredictionDespawnMarker};
pub use plugin::add_prediction_systems;
pub use predicted_history::{ComponentState, PredictionHistory};

use crate::client::components::{ComponentSyncMode, Confirmed};
use crate::client::prediction::resource::PredictionManager;
use crate::shared::replication::components::ShouldBePredicted;
use crate::shared::tick_manager::Tick;

/// This file is dedicated to running Prediction on entities.
/// On the client side, we run prediction on entities that are owned by the client.
/// The server is on tick 5, client is on tick 10, but we already applied the user's inputs for ticks 6->10.
///
/// When the server messages arrives (for tick 0), we:
/// - copy the server state (player movement, etc.) into the client's state
/// - reapply the last 10 frames, and re-apply the user's inputs in those 10 frames
/// the received server state. (server reconciliation)

/// Which means that for each predicted entity, we need:
/// - a buffer of the client inputs for the last RTT ticks
/// - a buffer of the components' states for the last RTT Ticks?  -> TO CHECK IF THERE WAS A MISPREDICTION AND WE NEED TO REPREDICT
/// - list of all the components that will be re-computed for reconciliation
mod despawn;
pub mod plugin;
pub mod predicted_history;
mod resource;
pub(crate) mod rollback;

/// Marks an entity that is being predicted by the client
#[derive(Component, Debug)]
pub struct Predicted {
    pub confirmed_entity: Entity,
    // TODO: add config about despawn behaviour here:
    //  - despawn immediately all components
    //  - leave the entity alive until the confirmed entity catches up to it and then it gets removed.
    //    - or do this only for certain components (audio, animation, particles..) -> mode on PredictedComponent
    // rollback_state: RollbackState,
}

#[derive(Resource)]
pub struct Rollback {
    pub(crate) state: RollbackState,
}

/// Resource that will track whether we should do rollback or not
/// (We have this as a resource because if any predicted entity needs to be rolled-back; we should roll back all predicted entities)
#[derive(Debug)]
pub enum RollbackState {
    Default,
    ShouldRollback {
        // tick we are setting (to record history)k
        current_tick: Tick,
    },
}

/// Spawn a predicted entity for each confirmed entity that has the `ShouldBePredicted` component added
/// The `Confirmed` entity could already exist because we share for prediction and interpolation.
// TODO: (although normally an entity shouldn't be both predicted and interpolated, so should we
//  instead panic if we find an entity that is both predicted and interpolated?)
pub fn spawn_predicted_entity(
    mut manager: ResMut<PredictionManager>,
    mut commands: Commands,
    mut confirmed_entities: Query<(Entity, Option<&mut Confirmed>), Added<ShouldBePredicted>>,
) {
    for (confirmed_entity, confirmed) in confirmed_entities.iter_mut() {
        // spawn a new predicted entity
        let predicted_entity_mut = commands.spawn(Predicted { confirmed_entity });
        let predicted_entity = predicted_entity_mut.id();

        // update the entity mapping
        manager
            .predicted_entity_map
            .remote_to_predicted
            .insert(confirmed_entity, predicted_entity);

        // add Confirmed to the confirmed entity
        // safety: we know the entity exists
        let mut confirmed_entity_mut = commands.get_entity(confirmed_entity).unwrap();
        if let Some(mut confirmed) = confirmed {
            confirmed.predicted = Some(predicted_entity);
        } else {
            confirmed_entity_mut.insert(Confirmed {
                predicted: Some(predicted_entity),
                interpolated: None,
            });
        }
        info!(
            "Spawn predicted entity {:?} for confirmed: {:?}",
            predicted_entity, confirmed_entity
        );
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("spawn_predicted_entity");
        }
    }
}
