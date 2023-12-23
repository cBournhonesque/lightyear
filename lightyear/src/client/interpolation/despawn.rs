use bevy::prelude::{Commands, EventReader, Query, RemovedComponents, ResMut};

use crate::client::components::{Confirmed, SyncComponent};
use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::interpolation_history::ConfirmedHistory;
use crate::client::interpolation::resource::InterpolationManager;
use crate::shared::events::ComponentRemoveEvent;

// Despawn logic:
// - despawning a predicted client entity:
//   - we add a DespawnMarker component to the entity
//   - all components other than ComponentHistory or Predicted get despawned, so that we can still check for rollbacks
//   - if the confirmed entity gets despawned, we despawn the predicted entity
//   - if the confirmed entity doesn't get despawned (during rollback, for example), it will re-add the necessary components to the predicted entity

// - TODO: despawning another client entity as a consequence from prediction, but we want to roll that back:
//   - maybe we don't do it, and we wait until we are sure (confirmed despawn) before actually despawning the entity

/// Remove the component from interpolated entities when it gets removed from confirmed
pub(crate) fn removed_components<C: SyncComponent>(
    mut commands: Commands,
    mut events: EventReader<ComponentRemoveEvent<C>>,
    query: Query<&Confirmed>,
) {
    for event in events.read() {
        if let Ok(confirmed) = query.get(*event.entity()) {
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
/// TODO: we should despawn interpolated only when it reaches the latest confirmed snapshot?
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
