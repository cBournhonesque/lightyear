use bevy::prelude::*;
use tracing::trace;

use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::interpolation::Interpolated;
use crate::prelude::TickManager;
use crate::shared::replication::components::ShouldBeInterpolated;

/// Spawn an interpolated entity for each confirmed entity that has the `ShouldBeInterpolated` component added
pub(crate) fn spawn_interpolated_entity(
    tick_manager: Res<TickManager>,
    config: Res<ClientConfig>,
    connection: Res<ConnectionManager>,
    mut commands: Commands,
    mut confirmed_entities: Query<(Entity, Option<&mut Confirmed>), Added<ShouldBeInterpolated>>,
) -> Result {
    for (confirmed_entity, confirmed) in confirmed_entities.iter_mut() {
        // skip if the entity already has an interpolated entity
        if confirmed.as_ref().is_some_and(|c| c.interpolated.is_some()) {
            continue;
        }
        let interpolated = commands.spawn(Interpolated { confirmed_entity }).id();

        // add Confirmed to the confirmed entity
        // safety: we know the entity exists
        let mut confirmed_entity_mut = commands.get_entity(confirmed_entity)?;
        if let Some(mut confirmed) = confirmed {
            confirmed.interpolated = Some(interpolated);
        } else {
            // get the confirmed tick for the entity
            // if we don't have it, something has gone very wrong
            trace!(
                "Adding Confirmed component on entity {:?} after we spawned Interpolated entity {:?}",
                confirmed_entity, interpolated
            );
            let confirmed_tick = connection
                .replication_receiver
                .get_confirmed_tick(confirmed_entity)
                // in most cases we will have a confirmed tick. The only case where we don't is if
                // the entity was originally spawned on this client, but then authority was removed
                // and we not want to add Interpolation
                .unwrap_or(tick_manager.tick());
            confirmed_entity_mut.insert(Confirmed {
                interpolated: Some(interpolated),
                predicted: None,
                tick: confirmed_tick,
            });
        }
        trace!(
            "Spawn interpolated entity {:?} for confirmed: {:?}",
            interpolated,
            confirmed_entity
        );
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("spawn_interpolated_entity").increment(1);
        }
    }
    Ok(())
}
