use bevy::prelude::*;

use crate::client::components::{Confirmed, SyncComponent};
use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::interpolation_history::ConfirmedHistory;

/// Remove the component from interpolated entities when it gets removed from confirmed
pub(crate) fn removed_components<C: SyncComponent>(
    trigger: Trigger<OnRemove, C>,
    mut commands: Commands,
    query: Query<&Confirmed>,
) {
    if let Ok(confirmed) = query.get(trigger.target()) {
        if let Some(interpolated) = confirmed.interpolated {
            if let Ok(mut entity) = commands.get_entity(interpolated) {
                entity.remove::<(C, ConfirmedHistory<C>, InterpolateStatus<C>)>();
            }
        }
    }
}

/// Despawn interpolated entities when the confirmed entity gets despawned
// TODO: we should despawn interpolated only when it reaches the latest confirmed snapshot?
//  I suppose  we could add a DespawnedMarker, and the entity would get despawned as soon as it reaches the end of interpolation...
//  not super priority but would be a nice to have
pub(crate) fn despawn_interpolated(
    trigger: Trigger<OnRemove, Confirmed>,
    query: Query<&Confirmed>,
    mut commands: Commands,
) -> Result {
    if let Some(interpolated) = query.get(trigger.target())?.interpolated {
        if let Ok(mut entity_mut) = commands.get_entity(interpolated) {
            entity_mut.despawn();
        }
    }
    Ok(())
}
