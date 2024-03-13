use bevy::prelude::{Commands, EventReader, Query, RemovedComponents, ResMut};

use crate::client::components::{Confirmed, SyncComponent};
use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::interpolation_history::ConfirmedHistory;
use crate::client::interpolation::resource::InterpolationManager;
use crate::shared::events::components::ComponentRemoveEvent;

/// Remove the component from interpolated entities when it gets removed from confirmed
pub(crate) fn removed_components<C: SyncComponent>(
    mut commands: Commands,
    mut events: EventReader<ComponentRemoveEvent<C>>,
    query: Query<&Confirmed>,
) {
    for event in events.read() {
        if let Ok(confirmed) = query.get(event.entity()) {
            if let Some(interpolated) = confirmed.interpolated {
                if let Some(mut entity) = commands.get_entity(interpolated) {
                    entity.remove::<C>();
                    entity.remove::<ConfirmedHistory<C>>();
                    entity.remove::<InterpolateStatus<C>>();
                }
            }
        }
    }
}

/// Despawn interpolated entities when the confirmed entity gets despawned
// TODO: we should despawn interpolated only when it reaches the latest confirmed snapshot?
//  might not be super straightforward because RemovedComponents lasts only one frame. I suppose
//  we could add a DespawnedMarker, and the entity would get despawned as soon as it reaches the end of interpolation...
//  not super priority but would be a nice to have
pub(crate) fn despawn_interpolated(
    mut manager: ResMut<InterpolationManager>,
    mut commands: Commands,
    mut query: RemovedComponents<Confirmed>,
) {
    for confirmed_entity in query.read() {
        if let Some(interpolated) = manager
            .interpolated_entity_map
            .confirmed_to_interpolated
            .remove(&confirmed_entity)
        {
            if let Some(mut entity_mut) = commands.get_entity(interpolated) {
                entity_mut.despawn();
            }
        }
    }
}
