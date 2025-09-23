use crate::interpolation_history::ConfirmedHistory;
use bevy_ecs::component::Component;
use bevy_ecs::prelude::With;
use bevy_ecs::{
    observer::Trigger,
    system::{Commands, Query},
    world::Remove,
};
use lightyear_core::interpolation::Interpolated;
use lightyear_replication::prelude::Confirmed;

/// Remove the component from interpolated entities when the confirmed component gets removed
// TODO: should the removal also be applied with interpolation delay?
pub(crate) fn removed_components<C: Component>(
    trigger: On<Remove, Confirmed<C>>,
    mut commands: Commands,
    query: Query<(), (With<Interpolated>, With<C>)>,
) {
    if query.get(trigger.entity).is_ok()
        && let Ok(mut entity) = commands.get_entity(trigger.target())
    {
        entity.try_remove::<(C, ConfirmedHistory<C>)>();
    }
}

// TODO: we should despawn interpolated only when it reaches the latest confirmed snapshot?
//  I suppose we could add a DespawnedMarker, and the entity would get despawned as soon as it reaches the end of interpolation...
//  not super priority but would be a nice to have
