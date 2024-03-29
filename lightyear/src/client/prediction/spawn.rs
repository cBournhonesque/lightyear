//! Logic to handle spawning Predicted entities
use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::metadata::GlobalMetadata;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::prelude::{Protocol, ShouldBePredicted};
use bevy::prelude::{Added, Commands, Entity, EventReader, Query, Ref, Res, ResMut};
use tracing::{debug, error};

/// Spawn a predicted entity for each confirmed entity that has the `ShouldBePredicted` component added
/// The `Confirmed` entity could already exist because we share the Confirmed component for prediction and interpolation.
// TODO: (although normally an entity shouldn't be both predicted and interpolated, so should we
//  instead panic if we find an entity that is both predicted and interpolated?)
pub(crate) fn spawn_predicted_entity<P: Protocol>(
    connection: Res<ConnectionManager<P>>,
    metadata: Res<GlobalMetadata>,
    mut manager: ResMut<PredictionManager>,
    mut commands: Commands,
    // get the list of entities who get ShouldBePredicted replicated from server
    mut should_be_predicted_added: EventReader<ComponentInsertEvent<ShouldBePredicted>>,
    mut confirmed_entities: Query<(Option<&mut Confirmed>, Ref<ShouldBePredicted>)>,
    mut predicted_entities: Query<&mut Predicted>,
) {
    for message in should_be_predicted_added.read() {
        let confirmed_entity = message.entity();

        if let Ok((confirmed, should_be_predicted)) = confirmed_entities.get_mut(confirmed_entity) {
            // TODO: improve this. Also that means we should run the pre-spawned system before this system AND have a flush...
            if confirmed.as_ref().is_some_and(|c| c.predicted.is_some()) {
                debug!("Skipping spawning prediction for pre-spawned player object (was already handled, we already have a predicted entity for this \
                      confirmed entity)");
                // special-case: pre-spawned player objects handled in a different function
                continue;
            }
            let mut predicted_entity = None;

            // check if we are in a pre-prediction scenario
            let mut should_spawn_predicted = true;
            if let Some(client_entity) = should_be_predicted.client_entity {
                if commands.get_entity(client_entity).is_none() {
                    error!(
                    "The pre-predicted entity has been deleted before we could receive the server's confirmation of it. \
                    This is probably because `EntityCommands::despawn()` has been called.\
                    On `Predicted` entities, you should call `EntityCommands::prediction_despawn()` instead."
                );
                    continue;
                }
                let client_id = should_be_predicted.client_id.unwrap();
                // make sure that the ShouldBePredicted is destined for this client
                if let Some(local_client_id) = metadata.client_id {
                    if client_id != local_client_id {
                        debug!(
                        local_client = ?local_client_id,
                        should_be_predicted_client = ?client_id,
                        "Received ShouldBePredicted component from server for an entity that is pre-predicted by another client: {:?}!", confirmed_entity);
                    } else {
                        // we have a pre-spawned predicted entity! instead of spawning a new predicted entity, we will
                        // just re-use the existing one!
                        should_spawn_predicted = false;
                        predicted_entity = Some(client_entity);
                        debug!(
                            "Re-use pre-spawned predicted entity {:?} for confirmed: {:?}",
                            predicted_entity, confirmed_entity
                        );
                        if let Ok(mut predicted) = predicted_entities.get_mut(client_entity) {
                            predicted.confirmed_entity = Some(confirmed_entity);
                        }

                        #[cfg(feature = "metrics")]
                        {
                            metrics::counter!("prespawn_predicted_entity").increment(1);
                        }
                    }
                }
            }

            if should_spawn_predicted {
                // we need to spawn a predicted entity for this confirmed entity
                let predicted_entity_mut = commands.spawn(Predicted {
                    confirmed_entity: Some(confirmed_entity),
                });
                predicted_entity = Some(predicted_entity_mut.id());
                debug!(
                    "Delayed prediction spawn! predicted entity {:?} for confirmed: {:?}",
                    predicted_entity, confirmed_entity
                );
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("spawn_predicted_entity").increment(1);
                }
            }

            // update the predicted entity mapping
            let predicted_entity = predicted_entity.unwrap();
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
                // TODO: this is the same as the current tick no? or maybe not because we could have received updates before the spawn
                //  and they are applied simultaneously
                // get the confirmed tick for the entity
                // if we don't have it, something has gone very wrong
                let confirmed_tick = connection
                    .replication_receiver
                    .get_confirmed_tick(confirmed_entity)
                    .unwrap();
                confirmed_entity_mut.insert(Confirmed {
                    predicted: Some(predicted_entity),
                    interpolated: None,
                    tick: confirmed_tick,
                });
            }
        } else {
            error!(
                "Received ShouldBePredicted component from server for an entity that does not exist: {:?}!", confirmed_entity
            );
        }
    }
}
