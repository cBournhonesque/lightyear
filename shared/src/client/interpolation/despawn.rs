use crate::client::interpolation::interpolate::InterpolateStatus;
use crate::client::interpolation::interpolation_history::ComponentHistory;
use crate::client::interpolation::InterpolatedComponent;
use crate::client::prediction::{Confirmed, Predicted, PredictedComponent};
use crate::plugin::events::{ComponentRemoveEvent, EntityDespawnEvent};
use crate::Entity;
use bevy::prelude::{Commands, EventReader, Query, RemovedComponents, ResMut, Resource, With};
use std::collections::HashMap;

// Despawn logic:
// - despawning a predicted client entity:
//   - we add a DespawnMarker component to the entity
//   - all components other than ComponentHistory or Predicted get despawned, so that we can still check for rollbacks
//   - if the confirmed entity gets despawned, we despawn the predicted entity
//   - if the confirmed entity doesn't get despawned (during rollback, for example), it will re-add the necessary components to the predicted entity

// - TODO: despawning another client entity as a consequence from prediction, but we want to roll that back:
//   - maybe we don't do it, and we wait until we are sure (confirmed despawn) before actually despawning the entity

#[derive(Resource)]
/// Mapping from confirmed entities to interpolated entities
/// Needed to despawn interpolated entities when the confirmed entity gets despawned
pub struct InterpolationMapping {
    pub confirmed_to_interpolated: HashMap<Entity, Entity>,
}

/// Remove the component from interpolated entities when it gets removed from confirmed
pub(crate) fn removed_components<C: InterpolatedComponent>(
    mut commands: Commands,
    mut events: EventReader<ComponentRemoveEvent<C>>,
    query: Query<&Confirmed>,
) {
    for event in events.read() {
        if let Ok(confirmed) = query.get(*event.entity()) {
            if let Some(interpolated) = confirmed.interpolated {
                if let Some(mut entity) = commands.get_entity(interpolated) {
                    entity.remove::<C>();
                    entity.remove::<ComponentHistory<C>>();
                    entity.remove::<InterpolateStatus<C>>();
                }
            }
        }
    }
}

/// Despawn interpolated entities when the confirmed entity gets despawned
pub(crate) fn despawn_interpolated(
    mut commands: Commands,
    mut mapping: ResMut<InterpolationMapping>,
    mut query: RemovedComponents<Confirmed>,
) {
    for entity in query.read() {
        if let Some(interpolated) = mapping.confirmed_to_interpolated.remove(&entity) {
            if let Some(mut entity_mut) = commands.get_entity(interpolated) {
                entity_mut.despawn();
            }
        }
    }
}
