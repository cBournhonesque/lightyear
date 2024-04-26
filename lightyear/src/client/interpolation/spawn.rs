use crate::_reexport::ShouldBeInterpolated;
use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::interpolation::resource::InterpolationManager;
use crate::client::interpolation::Interpolated;
use crate::prelude::{Protocol, Tick};
use bevy::prelude::{Added, Commands, Entity, Query, Res, ResMut};
use tracing::trace;

pub fn spawn_interpolated_entity(
    config: Res<ClientConfig>,
    connection: Res<ConnectionManager>,
    mut manager: ResMut<InterpolationManager>,
    mut commands: Commands,
    mut confirmed_entities: Query<(Entity, Option<&mut Confirmed>), Added<ShouldBeInterpolated>>,
) {
    for (confirmed_entity, confirmed) in confirmed_entities.iter_mut() {
        let interpolated = commands.spawn(Interpolated { confirmed_entity }).id();

        // update the entity mapping
        manager
            .interpolated_entity_map
            .confirmed_to_interpolated
            .insert(confirmed_entity, interpolated);

        // add Confirmed to the confirmed entity
        // safety: we know the entity exists
        let mut confirmed_entity_mut = commands.get_entity(confirmed_entity).unwrap();
        if let Some(mut confirmed) = confirmed {
            confirmed.interpolated = Some(interpolated);
        } else {
            // get the confirmed tick for the entity
            // if we don't have it, something has gone very wrong
            // let confirmed_tick = connection
            //     .replication_receiver
            //     .get_confirmed_tick(confirmed_entity)
            //     .unwrap();
            let confirmed_tick = Tick(0);
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
}
