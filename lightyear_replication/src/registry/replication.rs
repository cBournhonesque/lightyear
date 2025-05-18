use crate::prelude::ComponentReplicationConfig;
use crate::receive::TempWriteBuffer;
use crate::registry::registry::ComponentRegistry;
use crate::registry::{ComponentError, ComponentKind, ComponentNetId};
use bevy::ecs::component::{Component, ComponentId, Mutable};
use bevy::prelude::*;
use bytes::Bytes;
use lightyear_core::prelude::Tick;
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use tracing::{debug, trace};

#[derive(Debug, Clone, PartialEq)]
pub struct ReplicationMetadata {
    pub config: ComponentReplicationConfig,
    pub overrides_component_id: ComponentId,
    pub write: RawWriteFn,
    pub buffer_insert_fn: RawBufferInsertFn,
    pub remove: Option<RawBufferRemoveFn>,
}

type RawBufferRemoveFn = fn(&ComponentRegistry, &mut TempWriteBuffer);
pub type RawWriteFn = fn(
    &ComponentRegistry,
    &mut Reader,
    Tick,
    &mut EntityWorldMut,
    &mut ReceiveEntityMap,
) -> Result<(), ComponentError>;
type RawBufferInsertFn = fn(
    &ComponentRegistry,
    &mut Reader,
    Tick,
    &mut EntityWorldMut,
    &mut ReceiveEntityMap,
    &mut TempWriteBuffer,
) -> Result<(), ComponentError>;

impl ComponentRegistry {
    pub(crate) fn set_replication_fns<C: Component<Mutability = Mutable> + PartialEq>(
        &mut self,
        config: ComponentReplicationConfig,
        overrides_component_id: ComponentId,
    ) {
        self.replication_map.insert(
            ComponentKind::of::<C>(),
            ReplicationMetadata {
                config,
                overrides_component_id,
                write: Self::write::<C>,
                buffer_insert_fn: Self::buffer_insert::<C>,
                remove: Some(Self::buffer_remove::<C>),
            },
        );
    }

    /// Insert a batch of components on the entity
    ///
    /// This method will insert all the components simultaneously.
    /// If any component already existed on the entity, it will be updated instead of inserted.
    pub fn batch_insert(
        &self,
        component_bytes: Vec<Bytes>,
        entity_world_mut: &mut EntityWorldMut,
        tick: Tick,
        entity_map: &mut ReceiveEntityMap,
        temp_write_buffer: &mut TempWriteBuffer,
    ) -> Result<(), ComponentError> {
        component_bytes.into_iter().try_for_each(|b| {
            // TODO: reuse a single reader that reads through the entire message ?
            let mut reader = Reader::from(b);
            let net_id = ComponentNetId::from_bytes(&mut reader)?;
            let kind = self
                .kind_map
                .kind(net_id)
                .ok_or(ComponentError::NotRegistered)?;
            let replication_metadata = self
                .replication_map
                .get(kind)
                .ok_or(ComponentError::MissingReplicationFns)?;
            // buffer the component data into the temporary buffer so that
            // all components can be inserted at once
            (replication_metadata.buffer_insert_fn)(
                self,
                &mut reader,
                tick,
                entity_world_mut,
                entity_map,
                temp_write_buffer,
            )?;
            Ok::<(), ComponentError>(())
        })?;

        // TODO: sort by component id for cache efficiency!
        //  maybe it's not needed because on the server side we iterate through archetypes in a deterministic order?
        // # Safety
        // - Each [`ComponentId`] is from the same world as [`EntityWorldMut`]
        // - Each [`OwningPtr`] is a valid reference to the type represented by [`ComponentId`]
        //   (the data is store in self.raw_bytes)

        trace!(?temp_write_buffer.component_ids, "Inserting components into entity");
        // SAFETY: we call `buffer_insert_raw_ptrs` inside the `buffer_insert_fn` function
        unsafe { temp_write_buffer.batch_insert(entity_world_mut) };
        Ok(())
    }

    /// SAFETY: the ReadWordBuffer must contain bytes corresponding to the correct component type
    pub fn raw_write(
        &self,
        reader: &mut Reader,
        entity_world_mut: &mut EntityWorldMut,
        tick: Tick,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<ComponentKind, ComponentError> {
        let net_id = ComponentNetId::from_bytes(reader)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(ComponentError::NotRegistered)?;
        let replication_metadata = self
            .replication_map
            .get(kind)
            .ok_or(ComponentError::MissingReplicationFns)?;
        (replication_metadata.write)(self, reader, tick, entity_world_mut, entity_map)?;
        Ok(*kind)
    }

    /// Method that buffers a pointer to the component data that will be inserted
    /// in the entity inside `self.raw_bytes`
    pub fn buffer_insert<C: Component<Mutability = Mutable> + PartialEq>(
        &self,
        reader: &mut Reader,
        _tick: Tick,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut ReceiveEntityMap,
        temp_write_buffer: &mut TempWriteBuffer,
    ) -> Result<(), ComponentError> {
        let kind = ComponentKind::of::<C>();
        let component_id = self
            .kind_to_component_id
            .get(&kind)
            .ok_or(ComponentError::NotRegistered)?;
        let component = self.raw_deserialize::<C>(reader, entity_map)?;
        let entity = entity_world_mut.id();
        debug!(
            "Insert component {} to entity {entity:?}",
            core::any::type_name::<C>()
        );

        // if the component is already on the entity, no need to insert
        if let Some(mut c) = entity_world_mut.get_mut::<C>() {
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
            // TODO: add safety comment
            unsafe { temp_write_buffer.buffer_insert_raw_ptrs::<C>(component, *component_id) };
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

    pub fn write<C: Component<Mutability = Mutable> + PartialEq>(
        &self,
        reader: &mut Reader,
        _tick: Tick,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), ComponentError> {
        debug!(
            "Writing component {} to entity",
            core::any::type_name::<C>()
        );
        let component = self.raw_deserialize::<C>(reader, entity_map)?;
        if let Some(mut c) = entity_world_mut.get_mut::<C>() {
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
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("replication::receive::component::insert").increment(1);
                metrics::counter!(format!(
                    "replication::receive::component::{}::insert",
                    core::any::type_name::<C>()
                ))
                .increment(1);
            }
            entity_world_mut.insert(component);
        }
        Ok(())
    }

    pub fn batch_remove(
        &self,
        net_ids: Vec<ComponentNetId>,
        entity_world_mut: &mut EntityWorldMut,
        tick: Tick,
        temp_write_buffer: &mut TempWriteBuffer,
    ) {
        for net_id in net_ids {
            let kind = self.kind_map.kind(net_id).expect("unknown component kind");
            let replication_metadata = self
                .replication_map
                .get(kind)
                .expect("the component is not part of the protocol");
            let remove_fn = replication_metadata
                .remove
                .expect("the component does not have a remove function");
            remove_fn(self, temp_write_buffer);
        }

        entity_world_mut.remove_by_ids(&temp_write_buffer.component_ids);
        temp_write_buffer.component_ids.clear();
    }

    /// Prepare for a component being removed
    /// We don't actually remove the component here, we just push the ComponentId to the `component_ids` vector
    /// so that they can all be removed at the same time
    pub fn buffer_remove<C: Component>(&self, temp_write_buffer: &mut TempWriteBuffer) {
        let kind = ComponentKind::of::<C>();
        let component_id = self.kind_to_component_id.get(&kind).unwrap();
        temp_write_buffer.component_ids.push(*component_id);
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
