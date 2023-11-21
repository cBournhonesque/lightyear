use crate::client::prediction::{Confirmed, Predicted};
use crate::tick::Tick;
use crate::{Client, Protocol, World};
use bevy::ecs::system::{Command, EntityCommands};
use bevy::prelude::{Commands, Component, Entity};
use std::marker::PhantomData;

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
                predicted_entity_to_despawn = Some(confirmed.predicted);
                // if this is a confirmed entity
                // despawn both predicted and confirmed
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
