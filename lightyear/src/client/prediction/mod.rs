//! Handles client-side prediction
use std::fmt::Debug;

use bevy::prelude::{
    Added, Commands, Component, DetectChanges, Entity, EventReader, Query, Ref, ResMut, Resource,
    With, Without,
};
use tracing::{debug, error, info};

pub use despawn::{PredictionCommandsExt, PredictionDespawnMarker};
pub use plugin::add_prediction_systems;
pub use predicted_history::{ComponentState, PredictionHistory};

use crate::client::components::{ComponentSyncMode, Confirmed};
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::resource::PredictionManager;
use crate::protocol::Protocol;
use crate::shared::replication::components::{Replicate, ShouldBePredicted};
use crate::shared::tick_manager::Tick;

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
pub(crate) fn clean_prespawned_entity<P: Protocol>(
    mut commands: Commands,
    pre_predicted_entities: Query<Entity, With<ShouldBePredicted>>,
) {
    for entity in pre_predicted_entities.iter() {
        debug!("removing replicate from pre-spawned entity");
        commands
            .entity(entity)
            .remove::<Replicate<P>>()
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
        // check if we are in a pre-prediction scenario
        if let Some(client_entity) = should_be_predicted.client_entity {
            // NOTE: the check that the entity exists is necessary, because we could have
            // Client 1 spawn a pre-predicted entity (and attaches ShouldBePredicted)
            // But all servers want to predict that pre-predicted entity.

            // How do we distinguish that only for client-1 the entity is pre-predicted?
            // 1) Maybe on receive-side we can add the original client as Target
            // 2) Maybe just check that the client already has an entity with ShouldBePredicted (not perfect, because
            //    multiple clients could have the same pre-predicted entity)
            // 3) Maybe add the possibility to replicate a component differently to different clients, and we only replicate
            //    ShouldBePredicted to the original client
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
