//! Map between local and remote entities

use crate::reader::{ReadVarInt, Reader};
use crate::varint::varint_len;
use crate::writer::WriteInteger;
use crate::{SerializationError, ToBytes};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::entity::{Entity, EntityGeneration, EntityIndex};
use bevy_ecs::entity::{EntityMapper, hash_map::EntityHashMap};
use bevy_ecs::world::{EntityWorldMut, World};
use bevy_reflect::Reflect;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};

const MARKED: u64 = 1 << 63;

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
            debug!("Failed to map entity {entity:?}. Map: {self:?}");
            entity
        })
    }

    fn set_mapped(&mut self, source: Entity, target: Entity) {
        self.0.set_mapped(source, target);
    }
}

#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct SendEntityMap(pub(crate) EntityHashMap<Entity>);

impl SendEntityMap {
    /// Read-only entity lookup used while serializing.
    ///
    /// Serialization never inserts mappings, so this shared-access version is
    /// sufficient for the send path and can be called through a shared borrow.
    pub fn get_mapped_shared(&self, entity: Entity) -> Entity {
        let mut view = SendMapView {
            shared: None,
            local: self,
        };
        view.get_mapped(entity)
    }
}

impl EntityMapper for SendEntityMap {
    /// Try to map the entity using the map, or return the initial entity if it doesn't work
    fn get_mapped(&mut self, entity: Entity) -> Entity {
        self.get_mapped_shared(entity)
    }

    fn set_mapped(&mut self, source: Entity, target: Entity) {
        trace!(
            target: "lightyear_debug::entity",
            kind = "entity_map_insert",
            direction = "send",
            source_entity = ?source,
            remote_entity = ?target,
            "inserted send entity mapping"
        );
        self.0.insert(source, target);
    }
}

#[derive(Default, Debug, Reflect, Deref, DerefMut)]
pub struct ReceiveEntityMap(pub(crate) EntityHashMap<Entity>);

impl ReceiveEntityMap {
    /// Read-only entity lookup used while deserializing.
    ///
    /// Deserialization never inserts mappings, so this shared-access version is
    /// sufficient for the receive path and can be called through a shared borrow.
    pub fn get_mapped_shared(&self, entity: Entity) -> Entity {
        let mut view = ReceiveMapView {
            shared: None,
            local: self,
        };
        view.get_mapped(entity)
    }
}

impl EntityMapper for ReceiveEntityMap {
    /// Map an entity from the remote World to the local World
    fn get_mapped(&mut self, entity: Entity) -> Entity {
        self.get_mapped_shared(entity)
    }

    fn set_mapped(&mut self, source: Entity, target: Entity) {
        trace!(
            target: "lightyear_debug::entity",
            kind = "entity_map_insert",
            direction = "receive",
            remote_entity = ?source,
            entity = ?target,
            "inserted receive entity mapping"
        );
        self.0.insert(source, target);
    }
}

/// Read-only send-side lookup that implements [`EntityMapper`] over shared access.
///
/// Holds only shared references and performs only reads, so sharing this view across
/// parallel tasks is sound. `set_mapped` is unsupported, mirroring `bevy_replicon`'s
/// send context: mapping code that runs during serialization must not insert mappings.
///
/// An optional shared map (e.g. replicon's, on client connections) is checked first;
/// the connection-local map is the fallback for entities the shared map never sees.
///
/// A hit maps the entity and marks it as mapped so the receive side does not map it
/// again; a miss sends the entity as-is and lets the receiver map it.
#[derive(Clone, Copy)]
pub struct SendMapView<'a> {
    /// Shared external map, checked before [`SendMapView::local`].
    /// `None` on server-side connections and without external replication.
    pub shared: Option<&'a EntityHashMap<Entity>>,
    /// Connection-local send map (fallback).
    pub local: &'a SendEntityMap,
}

impl SendMapView<'_> {
    /// Look up the remote entity for a local entity (send side).
    fn get_remote(&self, local: Entity) -> Option<Entity> {
        if let Some(remote) = self.shared.and_then(|shared| shared.get(&local).copied()) {
            return Some(remote);
        }
        self.local.0.get(&local).copied()
    }
}

impl EntityMapper for SendMapView<'_> {
    fn get_mapped(&mut self, entity: Entity) -> Entity {
        match self.get_remote(entity) {
            Some(mapped) => {
                trace!("Mapping entity {entity:?} to {mapped:?} in SendMapView!");
                trace!(
                    target: "lightyear_debug::entity",
                    kind = "entity_map_send",
                    direction = "send",
                    source_entity = ?entity,
                    remote_entity = ?mapped,
                    "mapped entity while serializing"
                );
                RemoteEntityMap::mark_mapped(mapped)
            }
            _ => {
                // otherwise just send the entity as is, and the receiver will map it
                entity
            }
        }
    }

    fn set_mapped(&mut self, _source: Entity, _target: Entity) {
        unimplemented!(
            "SendMapView is read-only; MapEntities impls used during serialization must not insert mappings"
        );
    }
}

/// Read-only receive-side lookup that implements [`EntityMapper`] over shared access.
///
/// Holds only shared references and performs only reads, so sharing this view across
/// parallel tasks is sound. `set_mapped` is unsupported, mirroring `bevy_replicon`'s
/// send context: mapping code that runs during deserialization must not insert mappings.
///
/// An optional shared map (e.g. replicon's, on client connections) is checked first;
/// the connection-local map is the fallback for entities the shared map never sees.
///
/// An entity already marked on the send side is used as-is; otherwise it is looked up,
/// and `Entity::PLACEHOLDER` is returned when no mapping exists.
#[derive(Clone, Copy)]
pub struct ReceiveMapView<'a> {
    /// Shared external map, checked before [`ReceiveMapView::local`].
    /// `None` on server-side connections and without external replication.
    pub shared: Option<&'a EntityHashMap<Entity>>,
    /// Connection-local receive map (fallback).
    pub local: &'a ReceiveEntityMap,
}

impl ReceiveMapView<'_> {
    /// Look up the local entity for a remote entity (receive side).
    fn get_local(&self, remote: Entity) -> Option<Entity> {
        if let Some(local) = self.shared.and_then(|shared| shared.get(&remote).copied()) {
            return Some(local);
        }
        self.local.0.get(&remote).copied()
    }
}

impl EntityMapper for ReceiveMapView<'_> {
    fn get_mapped(&mut self, entity: Entity) -> Entity {
        // if the entity was already mapped on the send side, we don't need to map it again
        // since it's the local world entity
        if RemoteEntityMap::is_mapped(entity) {
            let mapped = RemoteEntityMap::mark_unmapped(entity);
            trace!(
                target: "lightyear_debug::entity",
                kind = "entity_map_receive_preserialized",
                direction = "receive",
                remote_entity = ?entity,
                entity = ?mapped,
                "entity was already mapped before receive-side deserialization"
            );
            mapped
        } else {
            // if we don't find the entity, return Entity::PLACEHOLDER as an error
            match self.get_local(entity) {
                Some(mapped) => {
                    trace!(
                        target: "lightyear_debug::entity",
                        kind = "entity_map_receive",
                        direction = "receive",
                        remote_entity = ?entity,
                        entity = ?mapped,
                        "mapped entity while deserializing"
                    );
                    mapped
                }
                None => {
                    debug!("Receive: Failed to map entity {entity:?}");
                    trace!(
                        target: "lightyear_debug::entity",
                        kind = "entity_map_missing",
                        direction = "receive",
                        remote_entity = ?entity,
                        "missing receive entity mapping"
                    );
                    Entity::PLACEHOLDER
                }
            }
        }
    }

    fn set_mapped(&mut self, _source: Entity, _target: Entity) {
        unimplemented!(
            "ReceiveMapView is read-only; MapEntities impls used during deserialization must not insert mappings"
        );
    }
}

#[derive(Default, Debug, Reflect)]
/// Map between local and remote entities. (used mostly on client because it's when we receive entity updates)
pub struct RemoteEntityMap {
    pub remote_to_local: ReceiveEntityMap,
    pub local_to_remote: SendEntityMap,
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
    pub fn get_local(&self, remote_entity: Entity) -> Option<Entity> {
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
    pub fn to_remote(&self, local_entity: Entity) -> Entity {
        match self.local_to_remote.get(&local_entity) {
            Some(remote_entity) => Self::mark_mapped(*remote_entity),
            _ => local_entity,
        }
    }

    /// Get the remote entity corresponding to the local entity in the entity map
    #[inline]
    pub fn get_remote(&self, local_entity: Entity) -> Option<Entity> {
        self.local_to_remote.get(&local_entity).copied()
    }

    /// Get the corresponding local entity for a given remote entity, or create it if it doesn't exist.
    pub fn get_by_remote<'a>(
        &mut self,
        world: &'a mut World,
        remote_entity: Entity,
    ) -> Option<EntityWorldMut<'a>> {
        self.get_local(remote_entity)
            .and_then(|e| world.get_entity_mut(e).ok())
    }

    /// Remove the entity from our mapping and return the local entity
    pub fn remove_by_remote(&mut self, remote_entity: Entity) -> Option<Entity> {
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

    #[allow(unused)]
    pub(crate) fn is_empty(&self) -> bool {
        self.remote_to_local.is_empty() && self.local_to_remote.is_empty()
    }

    pub fn clear(&mut self) {
        self.local_to_remote.clear();
        self.remote_to_local.clear();
    }
}

/// Serialize Entity as two varints for the index and generation (because they will probably be low).
/// Revisit this when relations comes out
impl ToBytes for Entity {
    // see details in `to_bytes`
    fn bytes_len(&self) -> usize {
        let mut index = u64::from(self.index_u32()) << 2;
        let is_mapped = RemoteEntityMap::is_mapped(*self);
        let unmarked = RemoteEntityMap::mark_unmapped(*self);

        let generation = unmarked.generation();
        let is_first_generation = generation == EntityGeneration::FIRST;
        index |= is_first_generation as u64;
        index |= (is_mapped as u64) << 1;

        let mut len = varint_len(index);
        if !is_first_generation {
            len += varint_len(generation.to_bits() as u64);
        }
        len
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        // the entity's bit pattern is:
        // - first 32 bits: generation. Lightyear uses the highest bit (generation 1^31) to indicate if the entity was mapped
        // - second 32 bits: index. The index cannot be u32::MAX

        // We will use 2 bits to indicate if:
        // - bit 1: if the generation is EntityGeneration::FIRST or not
        // - bit 2: if the entity generation has been
        // we put these bits at the end (low bits) since we use var int encoding
        let mut index = u64::from(self.index_u32()) << 2;
        let is_mapped = RemoteEntityMap::is_mapped(*self);
        let unmarked = RemoteEntityMap::mark_unmapped(*self);

        // we will use a second bit to indicate if the entity is mapped or not
        let generation = unmarked.generation();
        let is_first_generation = generation == EntityGeneration::FIRST;
        index |= is_first_generation as u64;
        index |= (is_mapped as u64) << 1;

        buffer.write_varint(index)?;
        if !is_first_generation {
            buffer.write_varint(generation.to_bits() as u64)?;
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let index = buffer.read_varint()?;
        let is_first_generation = (index & 1) != 0;
        let is_mapped = (index & 2) != 0;

        let generation = if !is_first_generation {
            u32::try_from(buffer.read_varint()?).map_err(|_| SerializationError::InvalidValue)?
        } else {
            0
        };
        let row = u32::try_from(index >> 2).map_err(|_| SerializationError::InvalidValue)?;
        let row = EntityIndex::from_raw_u32(row).ok_or(SerializationError::InvalidValue)?;
        let generation = EntityGeneration::from_bits(generation);
        let entity = Entity::from_index_and_generation(row, generation);
        if is_mapped {
            Ok(RemoteEntityMap::mark_mapped(entity))
        } else {
            Ok(entity)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ToBytes;
    use crate::entity_map::{
        ReceiveEntityMap, ReceiveMapView, RemoteEntityMap, SendEntityMap, SendMapView,
    };
    use crate::reader::Reader;
    use crate::writer::Writer;
    use bevy_ecs::entity::{
        Entity, EntityGeneration, EntityIndex, EntityMapper, hash_map::EntityHashMap,
    };
    use test_log::test;

    fn local_pair() -> (SendEntityMap, ReceiveEntityMap, Entity, Entity) {
        let local = Entity::from_raw_u32(1).unwrap();
        let remote = Entity::from_raw_u32(2).unwrap();
        let mut send = SendEntityMap::default();
        send.0.insert(local, remote);
        let mut receive = ReceiveEntityMap::default();
        receive.0.insert(remote, local);
        (send, receive, local, remote)
    }

    #[test]
    fn test_send_view_marks_hits_and_passes_through_misses() {
        let (send, _, local, remote) = local_pair();
        let unknown = Entity::from_raw_u32(3).unwrap();
        let mut view = SendMapView {
            shared: None,
            local: &send,
        };
        // hit: mapped and marked so the receiver skips lookup
        let mapped = view.get_mapped(local);
        assert!(RemoteEntityMap::is_mapped(mapped));
        assert_eq!(RemoteEntityMap::mark_unmapped(mapped), remote);
        // miss: sent as-is for the receiver to map
        assert_eq!(view.get_mapped(unknown), unknown);
    }

    #[test]
    fn test_send_view_prefers_shared_over_local() {
        let (send, _, local, _) = local_pair();
        let shared_remote = Entity::from_raw_u32(9).unwrap();
        let shared = EntityHashMap::from_iter([(local, shared_remote)]);
        let mut view = SendMapView {
            shared: Some(&shared),
            local: &send,
        };
        // shared hit wins over the local pair
        let mapped = view.get_mapped(local);
        assert_eq!(RemoteEntityMap::mark_unmapped(mapped), shared_remote);
    }

    #[test]
    fn test_send_view_shared_miss_falls_back_to_local() {
        let (send, _, local, remote) = local_pair();
        let other = Entity::from_raw_u32(7).unwrap();
        let shared_remote = Entity::from_raw_u32(9).unwrap();
        let shared = EntityHashMap::from_iter([(other, shared_remote)]);
        let mut view = SendMapView {
            shared: Some(&shared),
            local: &send,
        };
        // miss in shared, hit in local
        let mapped = view.get_mapped(local);
        assert_eq!(RemoteEntityMap::mark_unmapped(mapped), remote);
    }

    #[test]
    fn test_receive_view_unmarks_premapped_and_placeholders_misses() {
        let (_, receive, local, remote) = local_pair();
        let unknown = Entity::from_raw_u32(3).unwrap();
        let mut view = ReceiveMapView {
            shared: None,
            local: &receive,
        };
        // pre-mapped on the send side: used as-is without lookup
        assert_eq!(view.get_mapped(RemoteEntityMap::mark_mapped(local)), local);
        // hit: resolved through storage
        assert_eq!(view.get_mapped(remote), local);
        // miss: placeholder error
        assert_eq!(view.get_mapped(unknown), Entity::PLACEHOLDER);
    }

    #[test]
    fn test_receive_view_prefers_shared_over_local() {
        let (_, receive, _, remote) = local_pair();
        let shared_local = Entity::from_raw_u32(9).unwrap();
        let shared = EntityHashMap::from_iter([(remote, shared_local)]);
        let mut view = ReceiveMapView {
            shared: Some(&shared),
            local: &receive,
        };
        // shared hit wins over the local pair
        assert_eq!(view.get_mapped(remote), shared_local);
    }

    #[test]
    fn test_entity_serde_first_generation() {
        let e = Entity::from_index_and_generation(
            EntityIndex::from_raw_u32(1).unwrap(),
            EntityGeneration::FIRST,
        );

        let mut writer = Writer::with_capacity(100);
        e.to_bytes(&mut writer).unwrap();
        // entities of the first generation only serialize the row
        assert_eq!(writer.len(), 1);
        let mut reader = Reader::from(writer.take_written());
        let serde_e = Entity::from_bytes(&mut reader).unwrap();
        assert_eq!(e, serde_e);
    }

    #[test]
    fn test_entity_serde_non_first_generation() {
        let e = Entity::from_index_and_generation(
            EntityIndex::from_raw_u32(1).unwrap(),
            EntityGeneration::from_bits(1),
        );

        let mut writer = Writer::with_capacity(100);
        e.to_bytes(&mut writer).unwrap();
        // both the row and generation are serialized
        assert_eq!(writer.len(), 2);
        let mut reader = Reader::from(writer.take_written());
        let serde_e = Entity::from_bytes(&mut reader).unwrap();
        assert_eq!(e, serde_e);
    }

    #[test]
    fn test_entity_serde_mapped_first_generation() {
        let entity = Entity::from_raw_u32(10).unwrap();
        assert!(!RemoteEntityMap::is_mapped(entity));
        let entity_mapped = RemoteEntityMap::mark_mapped(entity);
        assert!(RemoteEntityMap::is_mapped(entity_mapped));

        let mut writer = Writer::with_capacity(100);
        entity_mapped.to_bytes(&mut writer).unwrap();
        // even with entity mapping, it only takes 1 bytes (since the `is_mapped` information is included in the row)
        assert_eq!(writer.len(), 1);
        let mut reader = Reader::from(writer.take_written());
        let serde_e = Entity::from_bytes(&mut reader).unwrap();
        assert_eq!(entity_mapped, serde_e);
    }

    #[test]
    fn test_entity_serde_mapped_non_first_generation() {
        let entity = Entity::from_index_and_generation(
            EntityIndex::from_raw_u32(10).unwrap(),
            EntityGeneration::from_bits(1),
        );
        assert!(!RemoteEntityMap::is_mapped(entity));
        let entity_mapped = RemoteEntityMap::mark_mapped(entity);
        assert!(RemoteEntityMap::is_mapped(entity_mapped));

        let mut writer = Writer::with_capacity(100);
        entity_mapped.to_bytes(&mut writer).unwrap();
        assert_eq!(writer.len(), 2);
        let mut reader = Reader::from(writer.take_written());
        let serde_e = Entity::from_bytes(&mut reader).unwrap();
        assert_eq!(entity_mapped, serde_e);
    }
}

//
// #[cfg(test)]
// mod tests {
//     use crate::client::components::Confirmed;
//     use crate::entity_map::RemoteEntityMap;
//     use crate::prelude::server::{Replicate, SyncTarget};
//     use crate::tests::protocol::*;
//     use crate::tests::stepper::BevyStepper;
//     use bevy::prelude::{default, Entity};
//
//     /// Test marking entities as mapped or not
//     #[test]
//     fn test_marking_entity() {
//         let entity = Entity::from_raw(1);
//         assert!(!RemoteEntityMap::is_mapped(entity));
//         let entity = RemoteEntityMap::mark_mapped(entity);
//         assert!(RemoteEntityMap::is_mapped(entity));
//     }
//
//     // An entity gets replicated from server to client,
//     // then a component gets removed from that entity on server,
//     // that component should also removed on client as well.
//     #[test]
//     fn test_replicated_entity_mapping() {
//         let mut stepper = BevyStepper::default();
//
//         // Create an entity on server
//         let server_entity = stepper
//             .server_app
//             .world_mut()
//             .spawn((ComponentSyncModeFull(0.0), Replicate::default()))
//             .id();
//         // we need to step twice because we run client before server
//         stepper.frame_step();
//         stepper.frame_step();
//
//         // Check that the entity is replicated to client
//         let client_entity = stepper
//             .client_app
//             .world()
//             .resource::<client::ConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_local(server_entity)
//             .unwrap();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()
//                 .entity(client_entity)
//                 .get::<ComponentSyncModeFull>()
//                 .unwrap(),
//             &ComponentSyncModeFull(0.0)
//         );
//
//         // Create an entity with a component that needs to be mapped
//         let server_entity_2 = stepper
//             .server_app
//             .world_mut()
//             .spawn((ComponentMapEntities(server_entity), Replicate::default()))
//             .id();
//         stepper.frame_step();
//         stepper.frame_step();
//
//         // Check that this entity was replicated correctly, and that the component got mapped
//         let client_entity_2 = stepper
//             .client_app
//             .world()
//             .resource::<client::ConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_local(server_entity_2)
//             .unwrap();
//         // the 'server entity' inside the Component4 component got mapped to the corresponding entity on the client
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()
//                 .entity(client_entity_2)
//                 .get::<ComponentMapEntities>()
//                 .unwrap(),
//             &ComponentMapEntities(client_entity)
//         );
//     }
//
//     /// Check that the EntityMap (used for PredictionEntityMap and InterpolationEntityMap)
//     /// doesn't map to Entity::PLACEHOLDER if the mapping fails.
//     ///
//     /// See: https://github.com/cBournhonesque/lightyear/issues/859
//     /// The reason is that we might have cases where we don't to map from Confirmed to Predicted,
//     /// for example if we spawn two entities C1 and C2 but only one of them is predicted.
//     #[test]
//     fn test_entity_map_no_mapping_found() {
//         let mut stepper = BevyStepper::default();
//         // s1 is predicted, s2 is not
//         let s1 = stepper
//             .server_app
//             .world_mut()
//             .spawn(Replicate {
//                 sync: SyncTarget {
//                     prediction: NetworkTarget::All,
//                     ..default()
//                 },
//                 ..default()
//             })
//             .id();
//         let s2 = stepper
//             .server_app
//             .world_mut()
//             .spawn(Replicate::default())
//             .id();
//         stepper.frame_step();
//         stepper.frame_step();
//         let c1_confirmed = stepper
//             .client_app
//             .world()
//             .resource::<client::ConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_local(s1)
//             .unwrap();
//         let c1_predicted = stepper
//             .client_app
//             .world()
//             .get::<Confirmed>(c1_confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//         let c2 = stepper
//             .client_app
//             .world()
//             .resource::<client::ConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_local(s2)
//             .unwrap();
//         // add a component on s1 that maps to an entity that doesn't have a predicted entity
//         stepper
//             .server_app
//             .world_mut()
//             .entity_mut(s1)
//             .insert(ComponentMapEntities(s2));
//         stepper.frame_step();
//         stepper.frame_step();
//
//         // check that the component is mapped correctly for the confirmed entities
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()
//                 .get::<ComponentMapEntities>(c1_confirmed)
//                 .unwrap(),
//             &ComponentMapEntities(c2)
//         );
//
//         // check that the component is unmapped for the predicted entities
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()
//                 .get::<ComponentMapEntities>(c1_predicted)
//                 .unwrap(),
//             &ComponentMapEntities(c2)
//         );
//     }
// }
