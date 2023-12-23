//! Handles client-side prediction
use std::fmt::Debug;

use bevy::prelude::{
    Added, Commands, Component, DetectChanges, Entity, EventReader, Query, Ref, ResMut, Resource,
    With,
};
use tracing::{debug, error, info};

pub use despawn::{PredictionCommandsExt, PredictionDespawnMarker};
pub use plugin::add_prediction_systems;
pub use predicted_history::{ComponentState, PredictionHistory};

use crate::client::components::{ComponentSyncMode, Confirmed};
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::resource::PredictionManager;
use crate::prelude::Replicate;
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
    // This is an option because we could spawn pre-predicted entities on the client that exist before we receive
    // the corresponding confirmed entity
    pub confirmed_entity: Option<Entity>,
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

/// For pre-spawned entities, we want to stop replicating as soon as the initial spawn message has been sent to the
/// server. Otherwise any predicted action we would do affect the server entity, even though we want the server to
/// have authority on the entity.
/// Therefore we will remove the `Replicate` component right after the first time we've sent a replicating message to the
/// server
pub(crate) fn clean_prespawned_entity(
    mut commands: Commands,
    mut pre_predicted_entities: Query<Entity, With<ShouldBePredicted>>,
) {
    for entity in pre_predicted_entities.iter() {
        debug!("removing replicate from pre-spawned entity");
        commands
            .entity(entity)
            .remove::<Replicate>()
            .remove::<ShouldBePredicted>()
            .insert(Predicted {
                confirmed_entity: None,
            });
    }
}

/// Spawn a predicted entity for each confirmed entity that has the `ShouldBePredicted` component added
/// The `Confirmed` entity could already exist because we share for prediction and interpolation.
// TODO: (although normally an entity shouldn't be both predicted and interpolated, so should we
//  instead panic if we find an entity that is both predicted and interpolated?)
pub(crate) fn spawn_predicted_entity(
    mut manager: ResMut<PredictionManager>,
    mut commands: Commands,
    mut confirmed_entities: Query<(Entity, Option<&mut Confirmed>, Ref<ShouldBePredicted>)>,
) {
    for (confirmed_entity, confirmed, should_be_predicted) in confirmed_entities.iter_mut() {
        if !should_be_predicted.is_added() {
            continue;
        }

        let predicted_entity: Entity;
        if let Some(client_entity) = should_be_predicted.client_entity {
            if client_entity == confirmed_entity {
                // this is the pre-spawned predicted entity, ignore
                continue;
            }
            if commands.get_entity(client_entity).is_none() {
                error!(
                    "The pre-predicted entity has been deleted before we could receive the server's confirmation of it.\
                    This is probably because `EntityCommands::despawn()` has been called.\
                    On `Predicted` entities, you should call `EntityCommands::prediction_despawn()` instead."
                );
                continue;
            }
            // we have a pre-spawned predicted entity! instead of spawning a new predicted entity, we will
            // just re-use the existing one!
            predicted_entity = client_entity;
            debug!(
                "Re-use pre-spawned predicted entity {:?} for confirmed: {:?}",
                predicted_entity, confirmed_entity
            );
            #[cfg(feature = "metrics")]
            {
                metrics::increment_counter!("prespawn_predicted_entity");
            }
        } else {
            // we need to spawn a predicted entity for this confirmed entity
            let predicted_entity_mut = commands.spawn(Predicted {
                confirmed_entity: Some(confirmed_entity),
            });
            predicted_entity = predicted_entity_mut.id();
            debug!(
                "Spawn predicted entity {:?} for confirmed: {:?}",
                predicted_entity, confirmed_entity
            );
            #[cfg(feature = "metrics")]
            {
                metrics::increment_counter!("spawn_predicted_entity");
            }
        }

        // update the entity mapping
        manager
            .predicted_entity_map
            .confirmed_to_predicted
            .insert(confirmed_entity, predicted_entity);

        info!(?manager.predicted_entity_map, "prediction map");

        // add Confirmed to the confirmed entity
        // safety: we know the entity exists
        let mut confirmed_entity_mut = commands.entity(confirmed_entity);
        confirmed_entity_mut.remove::<ShouldBePredicted>();

        if let Some(mut confirmed) = confirmed {
            confirmed.predicted = Some(predicted_entity);
        } else {
            confirmed_entity_mut.insert(Confirmed {
                predicted: Some(predicted_entity),
                interpolated: None,
            });
        }
    }
}
