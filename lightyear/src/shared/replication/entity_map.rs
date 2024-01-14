//! Map between local and remote entities
use anyhow::Context;
use bevy::prelude::{Entity, EntityWorldMut, World};
use bevy::utils::hashbrown::hash_map::Entry;
use bevy::utils::{EntityHashMap, EntityHashSet};

pub trait EntityMapper {
    /// Map an entity
    fn map(&self, entity: Entity) -> Option<Entity>;
}

impl<T: EntityMapper> EntityMapper for &T {
    #[inline]
    fn map(&self, entity: Entity) -> Option<Entity> {
        (*self).map(entity)
    }
}

impl EntityMapper for EntityHashMap<Entity, Entity> {
    #[inline]
    fn map(&self, entity: Entity) -> Option<Entity> {
        self.get(&entity).copied()
    }
}

#[derive(Default, Debug)]
/// Map between local and remote entities. (used mostly on client because it's when we receive entity updates)
pub struct RemoteEntityMap {
    remote_to_local: EntityHashMap<Entity, Entity>,
    local_to_remote: EntityHashMap<Entity, Entity>,
}

#[derive(Default, Debug)]
pub struct PredictedEntityMap {
    // map from the confirmed entity to the predicted entity
    // useful for despawning, as we won't have access to the Confirmed/Predicted components anymore
    pub(crate) confirmed_to_predicted: EntityHashMap<Entity, Entity>,
}

impl EntityMapper for PredictedEntityMap {
    #[inline]
    fn map(&self, entity: Entity) -> Option<Entity> {
        self.confirmed_to_predicted.get(&entity).copied()
    }
}

#[derive(Default, Debug)]
pub struct InterpolatedEntityMap {
    // map from the confirmed entity to the interpolated entity
    // useful for despawning, as we won't have access to the Confirmed/Interpolated components anymore
    pub(crate) confirmed_to_interpolated: EntityHashMap<Entity, Entity>,
}

impl EntityMapper for InterpolatedEntityMap {
    #[inline]
    fn map(&self, entity: Entity) -> Option<Entity> {
        self.confirmed_to_interpolated.get(&entity).copied()
    }
}

impl RemoteEntityMap {
    #[inline]
    pub fn insert(&mut self, remote_entity: Entity, local_entity: Entity) {
        self.remote_to_local.insert(remote_entity, local_entity);
        self.local_to_remote.insert(local_entity, remote_entity);
    }

    pub(crate) fn get_to_remote_mapper(&self) -> Box<dyn EntityMapper + '_> {
        Box::new(&self.local_to_remote)
    }

    // TODO: makke sure all calls to remote entity map use this to get the exact mapper
    pub(crate) fn get_to_local_mapper(&self) -> Box<dyn EntityMapper + '_> {
        Box::new(&self.remote_to_local)
    }

    #[inline]
    pub(crate) fn get_local(&self, remote_entity: Entity) -> Option<&Entity> {
        self.remote_to_local.get(&remote_entity)
    }

    #[inline]
    pub(crate) fn get_remote(&self, local_entity: Entity) -> Option<&Entity> {
        self.local_to_remote.get(&local_entity)
    }

    /// Get the corresponding local entity for a given remote entity, or create it if it doesn't exist.
    pub(super) fn get_by_remote<'a>(
        &mut self,
        world: &'a mut World,
        remote_entity: Entity,
    ) -> anyhow::Result<EntityWorldMut<'a>> {
        self.get_local(remote_entity)
            .and_then(|e| world.get_entity_mut(*e))
            .context("Failed to get local entity")
    }

    /// Get the corresponding local entity for a given remote entity, or create it if it doesn't exist.
    pub(super) fn get_by_remote_or_spawn<'a>(
        &mut self,
        world: &'a mut World,
        remote_entity: Entity,
    ) -> EntityWorldMut<'a> {
        match self.remote_to_local.entry(remote_entity) {
            Entry::Occupied(entry) => world.entity_mut(*entry.get()),
            Entry::Vacant(entry) => {
                let local_entity = world.spawn_empty();
                entry.insert(local_entity.id());
                self.local_to_remote
                    .insert(local_entity.id(), remote_entity);
                local_entity
            }
        }
    }

    pub(super) fn remove_by_remote(&mut self, remote_entity: Entity) -> Option<Entity> {
        let local_entity = self.remote_to_local.remove(&remote_entity);
        if let Some(local_entity) = local_entity {
            self.local_to_remote.remove(&local_entity);
        }
        local_entity
    }

    #[inline]
    pub fn to_local(&self) -> &EntityHashMap<Entity, Entity> {
        &self.remote_to_local
    }

    #[inline]
    pub fn to_remote(&self) -> &EntityHashMap<Entity, Entity> {
        &self.local_to_remote
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remote_to_local.is_empty() && self.local_to_remote.is_empty()
    }

    fn clear(&mut self) {
        self.local_to_remote.clear();
        self.remote_to_local.clear();
    }
}

impl EntityMapper for RemoteEntityMap {
    #[inline]
    fn map(&self, entity: Entity) -> Option<Entity> {
        self.get_local(entity).copied()
    }
}

/// Trait that Messages or Components must implement to be able to map entities
pub trait MapEntities<'a> {
    /// Map the entities inside the message or component from the remote World to the local World
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>);

    /// Get all the entities that are present in that message or component
    fn entities(&self) -> EntityHashSet<Entity>;
}

impl<'a> MapEntities<'a> for Entity {
    #[inline]
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {
        if let Some(local) = entity_mapper.map(*self) {
            *self = local;
        } else {
            panic!(
                "cannot map entity {:?} because it doesn't exist in the entity map!",
                self
            );
        }
    }

    #[inline]
    fn entities(&self) -> EntityHashSet<Entity> {
        EntityHashSet::from_iter(vec![*self])
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::prelude::client::*;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    // An entity gets replicated from server to client,
    // then a component gets removed from that entity on server,
    // that component should also removed on client as well.
    #[test]
    fn test_replicated_entity_mapping() -> anyhow::Result<()> {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            enable_replication: true,
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default().disable(false);
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        stepper.init();

        // Create an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((Component1(0.0), Replicate::default()))
            .id();
        // we need to step twice because we run client before server
        stepper.frame_step();
        stepper.frame_step();

        // Check that the entity is replicated to client
        let client_entity = *stepper
            .client_app
            .world
            .resource::<Connection>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .unwrap(),
            &Component1(0.0)
        );

        // Create an entity with a component that needs to be mapped
        let server_entity_2 = stepper
            .server_app
            .world
            .spawn((Component4(server_entity), Replicate::default()))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // Check that this entity was replicated correctly, and that the component got mapped
        let client_entity_2 = *stepper
            .client_app
            .world
            .resource::<Connection>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity_2)
            .unwrap();
        // the 'server entity' inside the Component4 component got mapped to the corresponding entity on the client
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity_2)
                .get::<Component4>()
                .unwrap(),
            &Component4(client_entity)
        );
        Ok(())
    }
}
