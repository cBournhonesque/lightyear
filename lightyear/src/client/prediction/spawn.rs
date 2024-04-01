//! Logic to handle spawning Predicted entities
use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::connection::client::ClientConnection;
use crate::prelude::{Protocol, ShouldBePredicted};
use crate::shared::replication::components::PrePredicted;
use bevy::prelude::{Added, Commands, Entity, EventReader, Query, Ref, Res, ResMut, With, Without};
use tracing::{debug, error, trace, warn};

/// Spawn a predicted entity for each confirmed entity that has the `ShouldBePredicted` component added
/// The `Confirmed` entity could already exist because we share the Confirmed component for prediction and interpolation.
// TODO: (although normally an entity shouldn't be both predicted and interpolated, so should we
//  instead panic if we find an entity that is both predicted and interpolated?)
pub(crate) fn spawn_predicted_entity<P: Protocol>(
    connection: Res<ConnectionManager<P>>,
    mut manager: ResMut<PredictionManager>,
    mut commands: Commands,
    // get the list of entities who get ShouldBePredicted replicated from server
    mut should_be_predicted_added: EventReader<ComponentInsertEvent<ShouldBePredicted>>,
    // only handle predicted that have ShouldBePredicted
    // (if the entity was handled by prespawn or prepredicted before, ShouldBePredicted gets removed)
    mut confirmed_entities: Query<Option<&mut Confirmed>, With<ShouldBePredicted>>,
) {
    for message in should_be_predicted_added.read() {
        let confirmed_entity = message.entity();
        warn!("Received entity with ShouldBePredicted from server: {confirmed_entity:?}");
        if let Ok(confirmed) = confirmed_entities.get_mut(confirmed_entity) {
            // we need to spawn a predicted entity for this confirmed entity
            let predicted_entity = commands
                .spawn(Predicted {
                    confirmed_entity: Some(confirmed_entity),
                })
                .id();
            warn!(
                "Spawning predicted entity {:?} for confirmed: {:?}",
                predicted_entity, confirmed_entity
            );
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("spawn_predicted_entity").increment(1);
            }

            // update the predicted entity mapping
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
            warn!("The confirmed entity {confirmed_entity:?} does not have ShouldBePredicted; it was probably handled by prespawn or prepredicted already");
        }
    }
}
