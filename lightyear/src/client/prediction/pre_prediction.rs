//! Module to handle pre-prediction logic (entities that are created on the client first),
//! then the ownership gets transferred to the server.
use bevy::prelude::*;
use tracing::{debug, error, trace, warn};

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::prespawn::PreSpawnedPlayerObjectSet;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::client::sync::client_is_synced;
use crate::connection::client::NetClient;
use crate::prelude::client::{ClientConnection, PredictionSet};
use crate::prelude::{ShouldBePredicted, Tick};
use crate::shared::replication::components::{PrePredicted, Replicate};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

#[derive(Default)]
pub(crate) struct PrePredictionPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PrePredictionSet {
    // PreUpdate Sets
    /// Handle receiving the confirmed entity for pre-predicted entities
    Spawn,
    // PostUpdate Sets
    /// Add the necessary information to the PrePrediction component (before replication)
    Fill,
    /// Remove the Replicate component from pre-predicted entities (after replication)
    Clean,
}

impl Plugin for PrePredictionPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PreUpdate,
            PrePredictionSet::Spawn.in_set(PredictionSet::SpawnPrediction),
        );
        app.configure_sets(
            PostUpdate,
            (
                PrePredictionSet::Fill,
                InternalReplicationSet::<ClientMarker>::All,
                PrePredictionSet::Clean,
            )
                .chain()
                .run_if(client_is_synced),
        );
        app.add_systems(
            PreUpdate,
            Self::spawn_pre_predicted_entity
                .after(PreSpawnedPlayerObjectSet::Spawn)
                .in_set(PrePredictionSet::Spawn),
        );
        app.add_systems(
            PostUpdate,
            (
                // fill in the client_entity and client_id for pre-predicted entities
                Self::fill_pre_prediction_data.in_set(PrePredictionSet::Fill),
                // clean-up the ShouldBePredicted components after we've sent them
                Self::clean_pre_predicted_entity.in_set(PrePredictionSet::Clean),
            ), // .run_if(is_connected),
        );
    }
}

impl PrePredictionPlugin {
    /// For `PrePredicted` entities, find the corresponding `Confirmed` entity. and add the `Confirmed` component to it.
    /// Also update the `Predicted` component on the pre-predicted entity.
    // TODO: (although normally an entity shouldn't be both predicted and interpolated, so should we
    //  instead panic if we find an entity that is both predicted and interpolated?)
    pub(crate) fn spawn_pre_predicted_entity(
        connection: Res<ConnectionManager>,
        mut manager: ResMut<PredictionManager>,
        mut commands: Commands,
        // get the list of entities who get PrePredicted replicated from server
        mut should_be_predicted_added: EventReader<ComponentInsertEvent<PrePredicted>>,
        mut confirmed_entities: Query<&PrePredicted>,
        mut predicted_entities: Query<&mut Predicted>,
    ) {
        for message in should_be_predicted_added.read() {
            let confirmed_entity = message.entity();
            debug!("Received entity with PrePredicted from server: {confirmed_entity:?}");
            if let Ok(pre_predicted) = confirmed_entities.get_mut(confirmed_entity) {
                let Some(predicted_entity) = pre_predicted.client_entity else {
                    error!("The PrePredicted component received from the server does not contain the pre-predicted entity!");
                    continue;
                };
                let Ok(mut predicted_entity_mut) = predicted_entities.get_mut(predicted_entity)
                else {
                    error!(
                    "The pre-predicted entity ({predicted_entity:?}) corresponding to the Confirmed entity ({confirmed_entity:?}) does not exist!"
                );
                    continue;
                };
                debug!(
                    "Re-use pre-spawned predicted entity {:?} for confirmed: {:?}",
                    predicted_entity, confirmed_entity
                );

                // update the predicted entity mapping
                manager
                    .predicted_entity_map
                    .get_mut()
                    .confirmed_to_predicted
                    .insert(confirmed_entity, predicted_entity);
                predicted_entity_mut.confirmed_entity = Some(confirmed_entity);
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("prespawn_predicted_entity").increment(1);
                }
                // add Confirmed to the confirmed entity
                // TODO: this is the same as the current tick no? or maybe not because we could have received updates before the spawn
                //  and they are applied simultaneously
                // get the confirmed tick for the entity
                // if we don't have it, something has gone very wrong
                let confirmed_tick = connection
                    .replication_receiver
                    .get_confirmed_tick(confirmed_entity)
                    .unwrap();
                commands
                    .entity(confirmed_entity)
                    .remove::<(ShouldBePredicted, PrePredicted)>()
                    .insert(Confirmed {
                        predicted: Some(predicted_entity),
                        interpolated: None,
                        tick: confirmed_tick,
                    });
            }
        }
    }

    /// If a client adds `PrePredicted` to an entity to perform pre-Prediction.
    /// We automatically add the extra needed information to the component.
    /// - client_entity: is needed to know which entity to use as the predicted entity
    /// - client_id: is needed in case the pre-predicted entity is predicted by other players upon replication
    pub(crate) fn fill_pre_prediction_data(
        connection: Res<ClientConnection>,
        mut query: Query<
            (Entity, &mut PrePredicted),
            // in unified mode, don't apply this to server->client entities
            Without<Confirmed>,
        >,
    ) {
        for (entity, mut pre_predicted) in query.iter_mut() {
            if pre_predicted.is_added() {
                debug!(
                client_id = ?connection.id(),
                entity = ?entity,
            "fill in pre-prediction info!");
                pre_predicted.client_entity = Some(entity);
            }
        }
    }

    /// For pre-spawned entities, we want to stop replicating as soon as the initial spawn message has been sent to the
    /// server. Otherwise any predicted action we would do affect the server entity, even though we want the server to
    /// have authority on the entity.
    /// Therefore we will remove the `Replicate` component right after the first time we've sent a replicating message to the
    /// server
    pub(crate) fn clean_pre_predicted_entity(
        mut commands: Commands,
        pre_predicted_entities: Query<Entity, (Added<PrePredicted>, Without<Confirmed>)>,
    ) {
        for entity in pre_predicted_entities.iter() {
            debug!(?entity, "removing replicate from pre-predicted entity");
            commands
                .entity(entity)
                .remove::<Replicate>()
                .insert((Predicted {
                    confirmed_entity: None,
                },));
        }
    }
}
