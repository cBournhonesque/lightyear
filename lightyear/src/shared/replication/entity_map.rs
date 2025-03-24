//! Map between local and remote entities
use bevy::ecs::entity::{hash_map::EntityHashMap, EntityMapper};
use bevy::prelude::{Deref, DerefMut, Entity, EntityWorldMut, World};
use bevy::reflect::Reflect;
use tracing::{debug, error, trace};

const MARKED: u64 = 1 << 62;

/// EntityMap that maps the entity if a mapping is present, or does nothing if not
///
/// The behaviour is different from the `SendEntityMap` or `RemoteEntityMap`, where
/// we return Entity::PLACEHOLDER if the mapping fails.
/// The reason is that `EntityMap` is used for Prediction/Interpolation mapping,
/// where we might not want to apply the mapping. For example, say we spawn C1 and C2
/// and only C1 is predicted to P1. If we add a component Mapped(C2) to C1, we will
/// try to do a mapping from C2 to P2 which doesn't exist. In that case we just want
/// to keep C2 in the component.
#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct EntityMap(pub(crate) EntityHashMap<Entity>);

impl EntityMapper for EntityMap {
    /// Try to map the entity using the map, or don't do anything if it fails
    fn get_mapped(&mut self, entity: Entity) -> Entity {
        self.0.get(&entity).copied().unwrap_or_else(|| {
            debug!("Failed to map entity {entity:?}");
            entity
        })
    }

    fn set_mapped(&mut self, source: Entity, target: Entity) {
        self.0.set_mapped(source, target);
    }
}

#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct SendEntityMap(pub(crate) EntityHashMap<Entity>);

impl EntityMapper for SendEntityMap {
    /// Try to map the entity using the map, or return the initial entity if it doesn't work
    fn get_mapped(&mut self, entity: Entity) -> Entity {
        // if we have the entity in our mapping, map it and mark it as mapped
        // so that on the receive side we don't map it again
        match self.0.get(&entity) { Some(mapped) => {
            trace!("Mapping entity {entity:?} to {mapped:?} in SendEntityMap!");
            RemoteEntityMap::mark_mapped(*mapped)
        } _ => {
            // otherwise just send the entity as is, and the receiver will map it
            entity
        }}
    }

    fn set_mapped(&mut self, source: Entity, target: Entity) {
        self.0.insert(source, target);
    }
}

#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct ReceiveEntityMap(pub(crate) EntityHashMap<Entity>);

impl EntityMapper for ReceiveEntityMap {
    /// Map an entity from the remote World to the local World
    fn get_mapped(&mut self, entity: Entity) -> Entity {
        // if the entity was already mapped on the send side, we don't need to map it again
        // since it's the local world entity
        if RemoteEntityMap::is_mapped(entity) {
            RemoteEntityMap::mark_unmapped(entity)
        } else {
            // if we don't find the entity, return Entity::PLACEHOLDER as an error
            self.0.get(&entity).copied().unwrap_or_else(|| {
                error!("Failed to map entity {entity:?}");
                Entity::PLACEHOLDER
            })
        }
    }

    fn set_mapped(&mut self, source: Entity, target: Entity) {
        self.0.insert(source, target);
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

    /// Get the local entity corresponding to the remote entity
    ///
    /// It's possible that the remote_entity was already mapped by the sender,
    /// in which case we don't want to map it again
    #[inline]
    pub(crate) fn get_local(&self, remote_entity: Entity) -> Option<Entity> {
        let unmapped = Self::mark_unmapped(remote_entity);
        if Self::is_mapped(remote_entity) {
            trace!("Received entity {unmapped:?} was already mapped, returning it as is");
            // the remote_entity is actually local, because it has already been mapped!
            // just remove the mapping bit
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
        match self.local_to_remote.get(&local_entity) { Some(remote_entity) => {
            Self::mark_mapped(*remote_entity)
        } _ => {
            local_entity
        }}
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
        } else { match self.remote_to_local.remove(&remote_entity) { Some(local) => {
            self.local_to_remote.remove(&local);
            return Some(local);
        } _ => {}}}
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
    use crate::client::components::Confirmed;
    use crate::prelude::server::{Replicate, SyncTarget};
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::{default, Entity};

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

    /// Check that the EntityMap (used for PredictionEntityMap and InterpolationEntityMap)
    /// doesn't map to Entity::PLACEHOLDER if the mapping fails.
    ///
    /// See: https://github.com/cBournhonesque/lightyear/issues/859
    /// The reason is that we might have cases where we don't to map from Confirmed to Predicted,
    /// for example if we spawn two entities C1 and C2 but only one of them is predicted.
    #[test]
    fn test_entity_map_no_mapping_found() {
        let mut stepper = BevyStepper::default();
        // s1 is predicted, s2 is not
        let s1 = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                sync: SyncTarget {
                    prediction: NetworkTarget::All,
                    ..default()
                },
                ..default()
            })
            .id();
        let s2 = stepper
            .server_app
            .world_mut()
            .spawn(Replicate::default())
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let c1_confirmed = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(s1)
            .unwrap();
        let c1_predicted = stepper
            .client_app
            .world()
            .get::<Confirmed>(c1_confirmed)
            .unwrap()
            .predicted
            .unwrap();
        let c2 = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(s2)
            .unwrap();
        // add a component on s1 that maps to an entity that doesn't have a predicted entity
        stepper
            .server_app
            .world_mut()
            .entity_mut(s1)
            .insert(ComponentMapEntities(s2));
        stepper.frame_step();
        stepper.frame_step();

        // check that the component is mapped correctly for the confirmed entities
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentMapEntities>(c1_confirmed)
                .unwrap(),
            &ComponentMapEntities(c2)
        );

        // check that the component is unmapped for the predicted entities
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentMapEntities>(c1_predicted)
                .unwrap(),
            &ComponentMapEntities(c2)
        );
    }
}
