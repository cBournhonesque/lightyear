use std::marker::PhantomData;

use bevy::ecs::system::{Command, EntityCommands};
use bevy::prelude::{Commands, Component, Entity, Query, With, World};

use crate::client::components::{Confirmed, SyncComponent};
use crate::client::prediction::Predicted;
use crate::client::resource::Client;
use crate::protocol::Protocol;
use crate::shared::tick_manager::Tick;

// Despawn logic:
// - despawning a predicted client entity:
//   - we add a DespawnMarker component to the entity
//   - all components other than ComponentHistory or Predicted get despawned, so that we can still check for rollbacks
//   - if the confirmed entity gets despawned, we despawn the predicted entity
//   - if the confirmed entity doesn't get despawned (during rollback, for example), it will re-add the necessary components to the predicted entity

// - TODO: despawning another client entity as a consequence from prediction, but we want to roll that back:
//   - maybe we don't do it, and we wait until we are sure (confirmed despawn) before actually despawning the entity

pub trait PredictionCommandsExt {
    fn prediction_despawn<P: Protocol>(&mut self);
}

pub struct PredictionDespawnCommand<P: Protocol> {
    entity: Entity,
    _marker: PhantomData<P>,
}

#[derive(Component, PartialEq, Debug)]
pub struct PredictionDespawnMarker {
    // TODO: do we need this?
    // TODO: it's pub just for integration tests right now
    pub death_tick: Tick,
}

impl<P: Protocol> Command for PredictionDespawnCommand<P> {
    fn apply(self, world: &mut World) {
        let client = world.get_resource::<Client<P>>().unwrap();
        let current_tick = client.tick();

        let mut predicted_entity_to_despawn: Option<Entity> = None;

        if let Some(mut entity) = world.get_entity_mut(self.entity) {
            if let Some(_) = entity.get::<Predicted>() {
                // if this is a predicted entity, do not despawn the entity immediately but instead
                // add a PredictionDespawn component to it to mark that it should be despawned as soon
                // as the confirmed entity catches up to it
                entity.insert(PredictionDespawnMarker {
                    death_tick: current_tick,
                });
                // TODO: if we want the death to be immediate on predicted,
                //  we should despawn all components immediately (except Predicted and History)
            } else if let Some(confirmed) = entity.get::<Confirmed>() {
                // if this is a confirmed entity
                // despawn both predicted and confirmed
                if let Some(predicted) = confirmed.predicted {
                    predicted_entity_to_despawn = Some(predicted);
                }
                entity.despawn();
            } else {
                panic!("this command should only be called for predicted or confirmed entities");
            }
        }
        if let Some(entity) = predicted_entity_to_despawn {
            world.despawn(entity);
        }
    }
}

impl PredictionCommandsExt for EntityCommands<'_, '_, '_> {
    fn prediction_despawn<P: Protocol>(&mut self) {
        let entity = self.id();
        self.commands().add(PredictionDespawnCommand {
            entity,
            _marker: PhantomData::<P>,
        })
    }
}

pub(crate) fn remove_component_for_despawn_predicted<C: SyncComponent>(
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
