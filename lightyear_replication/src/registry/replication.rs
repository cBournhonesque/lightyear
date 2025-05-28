use crate::prelude::ComponentReplicationConfig;
use crate::registry::buffered::BufferedEntity;
use crate::registry::registry::ComponentRegistry;
use crate::registry::{ComponentError, ComponentKind, ComponentNetId};
use bevy::ecs::component::{Component, ComponentId, Immutable, Mutable};
use bevy::prelude::*;
use bytes::Bytes;
use lightyear_core::prelude::Tick;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::{ContextDeserializeFns, ErasedSerializeFns};
use lightyear_serde::ToBytes;

#[derive(Debug, Clone, PartialEq)]
pub struct ReplicationMetadata {
    pub config: ComponentReplicationConfig,
    pub overrides_component_id: ComponentId,
    // BufferFn<C> is the typed function to insert component C
    pub(crate) inner_buffer: unsafe fn(),
    pub(crate) buffer: RawBufferFn,
    pub(crate) remove: Option<RawBufferRemoveFn>,
}

type RawBufferRemoveFn = fn(&ComponentRegistry, &mut BufferedEntity);

/// Function to perform a buffered insert of a component into the [`EntityWorldMut`]
type RawBufferFn = fn(
    &ReplicationMetadata,
    &ErasedSerializeFns,
    &mut Reader,
    Tick,
    &mut BufferedEntity,
    &mut ReceiveEntityMap,
) -> Result<(), ComponentError>;

impl ReplicationMetadata {
    pub(crate) fn new<C: Component>(
        config: ComponentReplicationConfig,
        overrides_component_id: ComponentId,
        buffer_fn: BufferFn<C>,
    ) -> Self {
        Self {
            config,
            overrides_component_id,
            inner_buffer: unsafe { core::mem::transmute::<BufferFn<C>, unsafe fn()>(buffer_fn) },
            buffer: Self::buffer::<C>,
            remove: Some(ComponentRegistry::buffer_remove::<C>),
        }
    }

    pub(crate) fn default_fns<C: Component<Mutability: GetWriteFns<C>> + PartialEq>(
        config: ComponentReplicationConfig,
        overrides_component_id: ComponentId,
    ) -> Self {
        Self::new(
            config,
            overrides_component_id,
            C::Mutability::buffer_fn(),
        )
    }

    pub(crate) fn buffer<C: Component>(
        &self,
        erased_serialize_fns: &ErasedSerializeFns,
        reader: &mut Reader,
        tick: Tick,
        entity_mut: &mut BufferedEntity,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), ComponentError> {
        let buffer_fn = unsafe { core::mem::transmute::<unsafe fn(), BufferFn<C>>(self.inner_buffer) };
        // SAFETY: the erased_deserialize is guaranteed to be valid for the type C
        let deserialize = unsafe { erased_serialize_fns.deserialize_fns::<_, C, C>() };
        buffer_fn(
            deserialize,
            reader,
            tick,
            entity_mut,
            entity_map,
        )
    }
}

impl ComponentRegistry {

    /// Insert a batch of components on the entity
    ///
    /// This method will insert all the components simultaneously.
    /// If any component already existed on the entity, it will be updated instead of inserted.
    pub(crate) fn buffer(
        &self,
        bytes: Bytes,
        entity_mut: &mut BufferedEntity,
        tick: Tick,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), ComponentError> {
        let mut reader = Reader::from(bytes);
        let net_id = ComponentNetId::from_bytes(&mut reader)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(ComponentError::NotRegistered)?;
        let replication_metadata = self
            .replication_map
            .get(kind)
            .ok_or(ComponentError::MissingReplicationFns)?;
        let erased_serialize_fns = self
            .serialize_fns_map
            .get(kind)
            .ok_or(ComponentError::MissingSerializationFns)?;
        (replication_metadata.buffer)(
            replication_metadata,
            erased_serialize_fns,
            &mut reader,
            tick,
            entity_mut,
            entity_map,
        )?;
        Ok::<(), ComponentError>(())
    }

    pub(crate) fn remove(
        &self,
        net_id: ComponentNetId,
        entity_mut: &mut BufferedEntity,
        tick: Tick,
    ) {
        let kind = self.kind_map.kind(net_id).expect("unknown component kind");
        let replication_metadata = self
            .replication_map
            .get(kind)
            .expect("the component is not part of the protocol");
        let remove_fn = replication_metadata
            .remove
            .expect("the component does not have a remove function");
        remove_fn(self, entity_mut);
    }

    /// Prepare for a component being removed
    /// We don't actually remove the component here, we just push the ComponentId to the `component_ids` vector
    /// so that they can all be removed at the same time
    pub(crate) fn buffer_remove<C: Component>(&self, entity_mut: &mut BufferedEntity) {
        let kind = ComponentKind::of::<C>();
        let component_id = self.kind_to_component_id.get(&kind).unwrap();
        entity_mut.buffered.remove(*component_id);
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::receive::component::remove").increment(1);
            metrics::counter!(format!(
                "replication::receive::component::{}::remove",
                core::any::type_name::<C>()
            ))
            .increment(1);
        }
    }
}

// pub fn register_component_send<C: Component>(app: &mut App, direction: NetworkDirection) {
//     let is_client = app.world().get_resource::<ClientConfig>().is_some();
//     let is_server = app.world().get_resource::<ServerConfig>().is_some();
//     match direction {
//         NetworkDirection::ClientToServer => {
//             if is_client {
//                 crate::client::replication::send::register_replicate_component_send::<C>(app);
//             }
//         }
//         NetworkDirection::ServerToClient => {
//             if is_server {
//                 crate::server::replication::send::register_replicate_component_send::<C>(app);
//             }
//
//         }
//         NetworkDirection::Bidirectional => {
//             register_component_send::<C>(app, NetworkDirection::ServerToClient);
//             register_component_send::<C>(app, NetworkDirection::ClientToServer);
//         }
//     }
// }

pub type BufferFn<C> = fn(
    deserialize: ContextDeserializeFns<ReceiveEntityMap, C, C>,
    reader: &mut Reader,
    tick: Tick,
    entity_mut: &mut BufferedEntity,
    entity_map: &mut ReceiveEntityMap,
) -> Result<(), ComponentError>;


pub trait GetWriteFns<C: Component> {
    fn buffer_fn() -> BufferFn<C>;
}

impl<C: Component<Mutability=Self> + PartialEq> GetWriteFns<C> for Mutable {
    fn buffer_fn() -> BufferFn<C> {
        default_buffer::<C>
    }
}


/// Default method to buffer a component for insertion
///
/// If the component already exists on the entity, it will be updated instead of inserted.
fn default_buffer<C: Component<Mutability = Mutable> + PartialEq>(
    deserialize: ContextDeserializeFns<ReceiveEntityMap, C, C>,
    reader: &mut Reader,
    _tick: Tick,
    entity_mut: &mut BufferedEntity,
    entity_map: &mut ReceiveEntityMap,
) -> Result<(), ComponentError> {
    let kind = ComponentKind::of::<C>();
    let component_id = entity_mut.component_id::<C>();
    let component = deserialize.deserialize(entity_map, reader)?;
    let entity = entity_mut.entity.id();
    debug!(
        "Insert component {} to entity {entity:?}",
        core::any::type_name::<C>()
    );

    // if the component is already on the entity, no need to insert
    if let Some(mut c) = entity_mut.entity.get_mut::<C>() {
        // TODO: when can we be in this situation? on authority change?
        // only apply the update if the component is different, to not trigger change detection
        if c.as_ref() != &component {
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("replication::receive::component::update").increment(1);
                metrics::counter!(format!(
                    "replication::receive::component::{}::update",
                    core::any::type_name::<C>()
                ))
                .increment(1);
            }
            *c = component;
        }
    } else {
        // SAFETY: the component_id matches the component
        unsafe { entity_mut.buffered.insert::<C>(component, component_id); }
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::receive::component::insert").increment(1);
            metrics::counter!(format!(
                "replication::receive::component::{}::insert",
                core::any::type_name::<C>()
            ))
            .increment(1);
        }
    }
    Ok(())
}

impl<C: Component<Mutability=Self> + PartialEq> GetWriteFns<C> for Immutable {
    fn buffer_fn() -> BufferFn<C> {
        default_immutable_buffer::<C>
    }
}


/// Default method to buffer a component for insertion
fn default_immutable_buffer<C: Component<Mutability = Immutable> + PartialEq>(
    deserialize: ContextDeserializeFns<ReceiveEntityMap, C, C>,
    reader: &mut Reader,
    _tick: Tick,
    entity_mut: &mut BufferedEntity,
    entity_map: &mut ReceiveEntityMap,
) -> Result<(), ComponentError> {
    let kind = ComponentKind::of::<C>();
    let component_id = entity_mut.component_id::<C>();
    let component = deserialize.deserialize(entity_map, reader)?;
    let entity = entity_mut.entity.id();
    debug!(
        "Insert component {} to entity {entity:?}",
        core::any::type_name::<C>()
    );
    if entity_mut.entity.get::<C>().is_none_or(|c| c != &component) {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::receive::component::insert").increment(1);
            metrics::counter!(format!(
                "replication::receive::component::{}::insert",
                core::any::type_name::<C>()
            ))
            .increment(1);
        }
        unsafe { entity_mut.buffered.insert::<C>(component, component_id); }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    // use super::*;
    // use crate::tests::protocol::{
    //     ComponentSyncModeFull, ComponentSyncModeOnce, ComponentSyncModeSimple,
    // };
    //
    // #[derive(Debug, Default, Clone, PartialEq, TypePath, Resource)]
    // struct Buffer(TempWriteBuffer);
    //
    // // TODO: this breaks because of https://github.com/bevyengine/bevy/pull/16219!
    // /// Make sure that the temporary buffer works properly even if it's being used recursively
    // /// because of observers
    // #[test]
    // fn test_recursive_temp_write_buffer() {
    //     let mut world = World::new();
    //     world.init_resource::<Buffer>();
    //
    //     world.add_observer(
    //         |trigger: Trigger<OnAdd, ComponentSyncModeFull>, mut commands: Commands| {
    //             let entity = trigger.target();
    //             commands.queue(move |world: &mut World| {
    //                 let component_id_once = world.register_component::<ComponentSyncModeOnce>();
    //                 let component_id_simple = world.register_component::<ComponentSyncModeSimple>();
    //                 let unsafe_world = world.as_unsafe_world_cell();
    //                 let mut buffer = unsafe { unsafe_world.get_resource_mut::<Buffer>() }.unwrap();
    //                 unsafe {
    //                     buffer.0.buffer_insert_raw_ptrs::<_>(
    //                         ComponentSyncModeOnce(1.0),
    //                         component_id_once,
    //                     )
    //                 }
    //                 unsafe {
    //                     buffer.0.buffer_insert_raw_ptrs::<_>(
    //                         ComponentSyncModeSimple(1.0),
    //                         component_id_simple,
    //                     )
    //                 }
    //                 // we insert both Once and Simple into the entity
    //                 let mut entity_world_mut =
    //                     unsafe { unsafe_world.world_mut() }.entity_mut(entity);
    //                 // SAFETY: we call `buffer_insert_raw_ptrs` above
    //                 unsafe { buffer.0.batch_insert(&mut entity_world_mut) };
    //             })
    //         },
    //     );
    //     world.add_observer(
    //         |trigger: Trigger<OnAdd, ComponentSyncModeOnce>, mut commands: Commands| {
    //             let entity = trigger.target();
    //             commands.queue(move |world: &mut World| {
    //                 let component_id = world.register_component::<ComponentSyncModeSimple>();
    //                 let unsafe_world = world.as_unsafe_world_cell();
    //                 let mut buffer = unsafe { unsafe_world.get_resource_mut::<Buffer>() }.unwrap();
    //                 unsafe {
    //                     buffer
    //                         .0
    //                         .buffer_insert_raw_ptrs::<_>(ComponentSyncModeSimple(1.0), component_id)
    //                 }
    //                 // we insert only Simple into the entity.
    //                 // we should NOT also be inserting the components that were previously in the buffer (Once) a second time
    //                 let mut entity_world_mut =
    //                     unsafe { unsafe_world.world_mut() }.entity_mut(entity);
    //                 // SAFETY: we call `buffer_insert_raw_ptrs` above
    //                 unsafe { buffer.0.batch_insert(&mut entity_world_mut) };
    //             })
    //         },
    //     );
    //     world.add_observer(
    //         |trigger: Trigger<OnInsert, ComponentSyncModeSimple>,
    //          mut query: Query<&mut ComponentSyncModeFull>| {
    //             if let Ok(mut comp) = query.get_mut(trigger.target()) {
    //                 comp.0 += 1.0;
    //             }
    //         },
    //     );
    //     world.spawn(ComponentSyncModeFull(0.0));
    //     world.flush();
    //
    //     // make sure that the ComponentSyncModeSimple was only inserted twice, not three times
    //     assert_eq!(
    //         world
    //             .query::<&ComponentSyncModeFull>()
    //             .single(&world)
    //             .unwrap()
    //             .0,
    //         2.0
    //     );
    // }
}
