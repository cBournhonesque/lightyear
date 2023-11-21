use crate::client::prediction::commands::PredictionDespawnMarker;
use crate::client::prediction::{Predicted, PredictedComponent};
use crate::plugin::events::EntityDespawnEvent;
use crate::Entity;
use bevy::prelude::{Commands, EventReader, Query, With};

// Despawn logic:
// - despawning a predicted client entity:
//   - we add a DespawnMarker component to the entity
//   - all components other than ComponentHistory or Predicted get despawned, so that we can still check for rollbacks
//   - if the confirmed entity gets despawned, we despawn the predicted entity
//   - if the confirmed entity doesn't get despawned (during rollback, for example), it will re-add the necessary components to the predicted entity

// - TODO: despawning another client entity as a consequence from prediction, but we want to roll that back:
//   - maybe we don't do it, and we wait until we are sure (confirmed despawn) before actually despawning the entity

pub(crate) fn remove_component_for_despawn_predicted<C: PredictedComponent>(
    mut commands: Commands,
    query: Query<Entity, (With<C>, With<Predicted>, With<PredictionDespawnMarker>)>,
) {
    for entity in query.iter() {
        // SAFETY: bevy guarantees that the entity exists
        commands.get_entity(entity).unwrap().remove::<C>();
    }
}

/// Remove the despawn marker: if during rollback the components are re-spawned, we don't want to re-despawn them again
pub(crate) fn remove_despawn_marker(
    mut commands: Commands,
    query: Query<Entity, With<PredictionDespawnMarker>>,
) {
    for entity in query.iter() {
        // SAFETY: bevy guarantees that the entity exists
        commands
            .get_entity(entity)
            .unwrap()
            .remove::<PredictionDespawnMarker>();
    }
}
