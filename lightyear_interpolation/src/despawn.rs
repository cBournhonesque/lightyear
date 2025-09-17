use crate::interpolate::InterpolateStatus;
use crate::interpolation_history::ConfirmedHistory;
use bevy_ecs::component::Component;
use bevy_ecs::{
    error::Result,
    observer::Trigger,
    system::{Commands, Query},
    world::Remove,
};
use lightyear_replication::prelude::Confirmed;

/// Remove the component from interpolated entities when it gets removed from confirmed
pub(crate) fn removed_components<C: Component>(
    trigger: On<Remove, C>,
    mut commands: Commands,
    query: Query<&Confirmed>,
) {
    if let Ok(confirmed) = query.get(trigger.entity)
        && let Some(interpolated) = confirmed.interpolated
        && let Ok(mut entity) = commands.get_entity(interpolated)
    {
        entity.try_remove::<(C, ConfirmedHistory<C>, InterpolateStatus<C>)>();
    }
}

/// Despawn interpolated entities when the confirmed entity gets despawned
// TODO: we should despawn interpolated only when it reaches the latest confirmed snapshot?
//  I suppose  we could add a DespawnedMarker, and the entity would get despawned as soon as it reaches the end of interpolation...
//  not super priority but would be a nice to have
pub(crate) fn despawn_interpolated(
    trigger: On<Remove, Confirmed>,
    query: Query<&Confirmed>,
    mut commands: Commands,
) -> Result {
    if let Ok(confirmed) = query.get(trigger.entity)
        && let Some(interpolated) = confirmed.interpolated
        && let Ok(mut entity_mut) = commands.get_entity(interpolated)
    {
        entity_mut.try_despawn();
    }
    Ok(())
}
