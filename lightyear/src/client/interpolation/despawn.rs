use bevy::prelude::{Commands, DespawnRecursiveExt, OnRemove, Query, ResMut, Trigger};

use crate::client::components::{Confirmed, SyncComponent};
use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::interpolation_history::ConfirmedHistory;
use crate::client::interpolation::resource::InterpolationManager;

/// Remove the component from interpolated entities when it gets removed from confirmed
pub(crate) fn removed_components<C: SyncComponent>(
    trigger: Trigger<OnRemove, C>,
    mut commands: Commands,
    query: Query<&Confirmed>,
) {
    if let Ok(confirmed) = query.get(trigger.entity()) {
        if let Some(interpolated) = confirmed.interpolated {
            if let Some(mut entity) = commands.get_entity(interpolated) {
                entity.remove::<(C, ConfirmedHistory<C>, InterpolateStatus<C>)>();
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
    trigger: Trigger<OnRemove, Confirmed>,
    mut manager: ResMut<InterpolationManager>,
    mut commands: Commands,
) {
    if let Some(interpolated) = manager
        .interpolated_entity_map
        .get_mut()
        .confirmed_to_interpolated
        .remove(&trigger.entity())
    {
        if let Some(entity_mut) = commands.get_entity(interpolated) {
            entity_mut.despawn_recursive();
        }
    }
}
