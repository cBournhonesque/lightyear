//! Map between local and remote entities
use bevy::ecs::entity::{EntityHashMap, EntityMapper};
use bevy::prelude::{Deref, DerefMut, Entity, EntityWorldMut, World};
use bevy::reflect::Reflect;

const MARKED: u64 = 1 << 62;

#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct EntityMap(pub(crate) EntityHashMap<Entity>);

impl EntityMapper for EntityMap {
    /// Try to map the entity using the map, or return the initial entity if it doesn't work
    fn map_entity(&mut self, entity: Entity) -> Entity {
        self.0.get(&entity).copied().unwrap_or(entity)
    }
}

#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct SendEntityMap(pub(crate) EntityHashMap<Entity>);

impl EntityMapper for SendEntityMap {
    /// Try to map the entity using the map, or return the initial entity if it doesn't work
    fn map_entity(&mut self, entity: Entity) -> Entity {
        // if the entity was mapped, mark it as mapped so we don't map it again on the receive side
        if let Some(mapped) = self.0.get(&entity) {
            RemoteEntityMap::mark_mapped(*mapped)
        } else {
            entity
        }
    }
}

#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct ReceiveEntityMap(pub(crate) EntityHashMap<Entity>);

impl EntityMapper for ReceiveEntityMap {
    /// Try to map the entity using the map, or return the initial entity if it doesn't work
    fn map_entity(&mut self, entity: Entity) -> Entity {
        // if the entity was already mapped on the send side, we don't need to map it again
        if RemoteEntityMap::is_mapped(entity) {
            RemoteEntityMap::mark_unmapped(entity)
        } else {
            self.0.get(&entity).copied().unwrap_or(entity)
        }
    }
}

#[derive(Default, Debug, Reflect)]
/// Map between local and remote entities. (used mostly on client because it's when we receive entity updates)
pub struct RemoteEntityMap {
    pub(crate) remote_to_local: ReceiveEntityMap,
    pub(crate) local_to_remote: SendEntityMap,
}

#[derive(Default, Debug, Reflect)]
pub struct PredictedEntityMap {
    /// Map from the confirmed entity to the predicted entity
    /// useful for despawning, as we won't have access to the Confirmed/Predicted components anymore
    pub(crate) confirmed_to_predicted: EntityMap,
}

#[derive(Default, Debug, Reflect)]
pub struct InterpolatedEntityMap {
    // map from the confirmed entity to the interpolated entity
    // useful for despawning, as we won't have access to the Confirmed/Interpolated components anymore
    pub(crate) confirmed_to_interpolated: EntityMap,
}

impl RemoteEntityMap {
    /// Insert a new mapping between a remote entity and a local entity
    #[inline]
    pub fn insert(&mut self, remote_entity: Entity, local_entity: Entity) {
        self.remote_to_local.insert(remote_entity, local_entity);
        self.local_to_remote.insert(local_entity, remote_entity);
    }

    // pub(crate) fn get_to_remote_mapper(&self) -> Box<dyn EntityMapper + '_> {
    //     Box::new(&self.local_to_remote)
    // }
    //
    // // TODO: make sure all calls to remote entity map use this to get the exact mapper
    // pub(crate) fn get_to_local_mapper(&self) -> Box<dyn EntityMapper + '_> {
    //     Box::new(&self.remote_to_local)
    // }

    /// Get the local entity corresponding to the remote entity
    ///
    /// It's possible that the remote_entity was already mapped by the sender,
    /// in which case we don't want to map it again
    #[inline]
    pub(crate) fn get_local(&self, remote_entity: Entity) -> Option<Entity> {
        // the remote_entity is actually local, because it has already been mapped!
        let unmapped = Self::mark_unmapped(remote_entity);
        if Self::is_mapped(remote_entity) {
            return Some(unmapped);
        };
        self.remote_to_local.get(&unmapped).copied()
    }

    /// We want to map entities in two situations:
    /// - an entity has been replicated to use so we've added it in our Remote->Local mapping. When we receive an entity
    ///   from the sender, we want to check if the entity has been mapped before.
    /// - but in some situations the sender has already mapped the entity; maybe it's because the authority has changes,
    ///   or because the receiver is sending a message about an entity so it does the mapping locally. In which case we don't want
    ///   both the receiver and the sender to apply a mapping, because it wouldn't work.
    ///
    /// So we use a dead bit on the entity to mark it as mapped. If an entity is already marked as mapped, the receiver won't try
    /// to map it again
    pub(crate) const fn mark_mapped(entity: Entity) -> Entity {
        let mut bits = entity.to_bits();
        bits |= MARKED;
        Entity::from_bits(bits)
    }

    pub(crate) const fn mark_unmapped(entity: Entity) -> Entity {
        let mut bits = entity.to_bits();
        bits &= !MARKED;
        Entity::from_bits(bits)
    }

    /// Returns true if the entity already has been mapped
    pub(crate) const fn is_mapped(entity: Entity) -> bool {
        entity.to_bits() & MARKED != 0
    }

    /// Convert a local entity to a network entity that we can send
    /// We will try to map it to a remote entity if we can
    pub(crate) fn to_remote(&self, local_entity: Entity) -> Entity {
        if let Some(remote_entity) = self.local_to_remote.get(&local_entity) {
            Self::mark_mapped(*remote_entity)
        } else {
            local_entity
        }
    }

    /// Get the remote entity corresponding to the local entity in the entity map
    #[inline]
    pub(crate) fn get_remote(&self, local_entity: Entity) -> Option<Entity> {
        self.local_to_remote.get(&local_entity).copied()
    }

    /// Get the corresponding local entity for a given remote entity, or create it if it doesn't exist.
    pub(super) fn get_by_remote<'a>(
        &mut self,
        world: &'a mut World,
        remote_entity: Entity,
    ) -> Option<EntityWorldMut<'a>> {
        self.get_local(remote_entity)
            .and_then(|e| world.get_entity_mut(e).ok())
    }

    /// Remove the entity from our mapping and return the local entity
    pub(super) fn remove_by_remote(&mut self, remote_entity: Entity) -> Option<Entity> {
        // the entity is actually local, because it has already been mapped!
        if Self::is_mapped(remote_entity) {
            let local = Self::mark_unmapped(remote_entity);
            if let Some(remote) = self.local_to_remote.remove(&local) {
                self.remote_to_local.remove(&remote);
            }
            return Some(local);
        } else if let Some(local) = self.remote_to_local.remove(&remote_entity) {
            self.local_to_remote.remove(&local);
            return Some(local);
        }
        None
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remote_to_local.is_empty() && self.local_to_remote.is_empty()
    }

    fn clear(&mut self) {
        self.local_to_remote.clear();
        self.remote_to_local.clear();
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::server::Replicate;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::Entity;

    /// Test marking entities as mapped or not
    #[test]
    fn test_marking_entity() {
        let entity = Entity::from_raw(1);
        assert!(!RemoteEntityMap::is_mapped(entity));
        let entity = RemoteEntityMap::mark_mapped(entity);
        assert!(RemoteEntityMap::is_mapped(entity));
    }

    // An entity gets replicated from server to client,
    // then a component gets removed from that entity on server,
    // that component should also removed on client as well.
    #[test]
    fn test_replicated_entity_mapping() {
        let mut stepper = BevyStepper::default();

        // Create an entity on server
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((ComponentSyncModeFull(0.0), Replicate::default()))
            .id();
        // we need to step twice because we run client before server
        stepper.frame_step();
        stepper.frame_step();

        // Check that the entity is replicated to client
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<ComponentSyncModeFull>()
                .unwrap(),
            &ComponentSyncModeFull(0.0)
        );

        // Create an entity with a component that needs to be mapped
        let server_entity_2 = stepper
            .server_app
            .world_mut()
            .spawn((ComponentMapEntities(server_entity), Replicate::default()))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // Check that this entity was replicated correctly, and that the component got mapped
        let client_entity_2 = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity_2)
            .unwrap();
        // the 'server entity' inside the Component4 component got mapped to the corresponding entity on the client
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(client_entity_2)
                .get::<ComponentMapEntities>()
                .unwrap(),
            &ComponentMapEntities(client_entity)
        );
    }
}
