//! Logic to handle spawning Predicted entities
use crate::Predicted;
use crate::manager::PredictionManager;
use bevy_ecs::prelude::*;
use lightyear_connection::client::Connected;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_replication::prelude::{Confirmed, ReplicationReceiver, ShouldBePredicted};
#[allow(unused_imports)]
use tracing::{debug, warn};

/// Spawn a predicted entity for each confirmed entity that has the `ShouldBePredicted` component added
/// The `Confirmed` entity could already exist because we share the Confirmed component for prediction and interpolation.
// TODO: (although normally an entity shouldn't be both predicted and interpolated, so should we
//  instead panic if we find an entity that is both predicted and interpolated?)
pub(crate) fn spawn_predicted_entity(
    mut commands: Commands,
    // only handle predicted that have ShouldBePredicted
    // (if the entity was handled by prespawn or prepredicted before, ShouldBePredicted gets removed)
    mut confirmed_entities: Query<(Entity, Option<&mut Confirmed>), Added<ShouldBePredicted>>,
    link: Single<
        (&ReplicationReceiver, &LocalTimeline),
        (With<PredictionManager>, With<Connected>),
    >,
) {
    let (receiver, timeline) = link.into_inner();
    // TODO: should we check if the sender for the entity corresponds to the link?
    for (confirmed_entity, confirmed) in confirmed_entities.iter_mut() {
        // skip if the entity already has a predicted entity
        if confirmed.as_ref().is_some_and(|c| c.predicted.is_some()) {
            continue;
        }
        debug!("Received entity with ShouldBePredicted from server: {confirmed_entity:?}");
        // we need to spawn a predicted entity for this confirmed entity
        let predicted_entity = commands
            .spawn(Predicted {
                confirmed_entity: Some(confirmed_entity),
            })
            .id();
        debug!(
            "Spawning predicted entity {:?} for confirmed: {:?}",
            predicted_entity, confirmed_entity
        );
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("prediction::pre_predicted_spawn").increment(1);
        }

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
            let confirmed_tick = receiver
                .get_confirmed_tick(confirmed_entity)
                // in most cases we will have a confirmed tick. The only case where we don't is if
                // the entity was originally spawned on this client, but then authority was removed
                // and we not want to add Prediction
                .unwrap_or(timeline.tick());
            confirmed_entity_mut.insert(Confirmed {
                predicted: Some(predicted_entity),
                interpolated: None,
                tick: confirmed_tick,
            });
        }
    }
}
