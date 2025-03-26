use crate::channel::builder::ChannelDirection;
use crate::client::config::ClientConfig;
use crate::prelude::{ComponentRegistry, Tick};
use crate::protocol::component::{ComponentError, ComponentKind, ComponentNetId};
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::server::config::ServerConfig;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::replication::entity_map::ReceiveEntityMap;

use bevy::ecs::component::{Component, ComponentId, Mutable};
use bevy::prelude::*;
use bevy::ptr::OwningPtr;
use bytes::Bytes;
use core::alloc::Layout;
use core::ptr::NonNull;
use tracing::{debug, trace};

/// Temporary buffer to store component data that we want to insert
/// using `entity_world_mut.insert_by_ids`
#[derive(Debug, Default, Clone, PartialEq, TypePath)]
pub struct TempWriteBuffer {
    // temporary buffers to store the deserialized data to batch write
    // Raw storage where we can store the deserialized data bytes
    raw_bytes: Vec<u8>,
    // Positions of each component in the `raw_bytes` buffer
    component_ptrs_indices: Vec<usize>,
    // List of component ids
    component_ids: Vec<ComponentId>,
    // Position of the `component_ptr_indices` and `component_ids` list
    // This is needed because we can write into the buffer recursively.
    // For example if we write component A in the buffer, then call entity_mut_world.insert(A),
    // we might trigger an observer that inserts(B) in the buffer before it can be cleared
    cursor: usize,
}

impl TempWriteBuffer {
    fn is_empty(&self) -> bool {
        self.cursor == self.component_ids.len()
    }
    // TODO: also write a similar function for component removals, to handle recursive removals!

    /// Inserts the components that were buffered inside the EntityWorldMut
    ///
    /// SAFETY: `buffer_insert_raw_ptrs` must have been called beforehand
    pub(crate) unsafe fn batch_insert(&mut self, entity_world_mut: &mut EntityWorldMut) {
        if self.is_empty() {
            return;
        }
        // apply all commands from start_cursor to end
        // SAFETY: a value was insert in the cursor in a previous call to `buffer_insert_raw_ptrs`
        let start = self.cursor;
        // set the cursor position so that recursive calls only start reading the buffer from this
        // position
        self.cursor = self.component_ids.len();
        let start_index = self.component_ptrs_indices[start];
        // apply all buffer contents from `start` to the end
        unsafe {
            entity_world_mut.insert_by_ids(
                &self.component_ids[start..],
                self.component_ptrs_indices[start..].iter().map(|index| {
                    let ptr = NonNull::new_unchecked(self.raw_bytes.as_mut_ptr().add(*index));
                    OwningPtr::new(ptr)
                }),
            )
        };
        // clear the raw bytes that we inserted in the entity_world_mut
        self.component_ptrs_indices.drain(start..);
        self.component_ids.drain(start..);
        self.raw_bytes.drain(start_index..);
        self.cursor = start;
    }

    /// Store the component's raw bytes into a temporary buffer so that we can get an OwningPtr to it
    /// This function is called for all components that will be added to an entity, so that we can
    /// insert them all at once using `entity_world_mut.insert_by_ids`
    ///
    /// SAFETY:
    /// - the component C must match the `component_id `
    pub unsafe fn buffer_insert_raw_ptrs<C: Component>(
        &mut self,
        mut component: C,
        component_id: ComponentId,
    ) {
        let layout = Layout::new::<C>();
        // SAFETY: we are creating a pointer to the component data, which is non-null
        let ptr = unsafe { NonNull::new_unchecked(&mut component).cast::<u8>() };
        // make sure the Drop trait is not called when the `component` variable goes out of scope
        core::mem::forget(component);
        let count = layout.size();
        self.raw_bytes.reserve(count);
        let space = unsafe { NonNull::new_unchecked(self.raw_bytes.spare_capacity_mut()).cast::<u8>() };
        unsafe { space.copy_from_nonoverlapping(ptr, count) } ;
        let length = self.raw_bytes.len();
        // SAFETY: we are using the spare capacity of the Vec, so we know that the length is correct
        unsafe { self.raw_bytes.set_len(length + count) };
        self.component_ptrs_indices.push(length);
        self.component_ids.push(component_id);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplicationMetadata {
    pub direction: ChannelDirection,
    pub write: RawWriteFn,
    pub buffer_insert_fn: RawBufferInsertFn,
    pub remove: Option<RawBufferRemoveFn>,
}

type RawBufferRemoveFn = fn(&mut ComponentRegistry);
pub type RawWriteFn = fn(
    &ComponentRegistry,
    &mut Reader,
    Tick,
    &mut EntityWorldMut,
    &mut ReceiveEntityMap,
    &mut ConnectionEvents,
) -> Result<(), ComponentError>;
type RawBufferInsertFn = fn(
    &mut ComponentRegistry,
    &mut Reader,
    Tick,
    &mut EntityWorldMut,
    &mut ReceiveEntityMap,
    &mut ConnectionEvents,
) -> Result<(), ComponentError>;

impl ComponentRegistry {
    pub fn direction(&self, kind: ComponentKind) -> Option<ChannelDirection> {
        self.replication_map
            .get(&kind)
            .map(|metadata| metadata.direction)
    }

    pub fn set_replication_fns<C: Component<Mutability = Mutable> + PartialEq>(
        &mut self,
        world: &mut World,
        direction: ChannelDirection,
    ) {
        self.replication_map.insert(
            ComponentKind::of::<C>(),
            ReplicationMetadata {
                direction,
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
        &mut self,
        component_bytes: Vec<Bytes>,
        entity_world_mut: &mut EntityWorldMut,
        tick: Tick,
        entity_map: &mut ReceiveEntityMap,
        events: &mut ConnectionEvents,
    ) -> Result<(), ComponentError> {
        component_bytes.into_iter().try_for_each(|b| {
            // TODO: reuse a single reader that reads through the entire message ?
            let mut reader = Reader::from(b);
            let net_id =
                ComponentNetId::from_bytes(&mut reader).map_err(SerializationError::from)?;
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
                events,
            )?;
            Ok::<(), ComponentError>(())
        })?;

        // TODO: sort by component id for cache efficiency!
        //  maybe it's not needed because on the server side we iterate through archetypes in a deterministic order?
        // # Safety
        // - Each [`ComponentId`] is from the same world as [`EntityWorldMut`]
        // - Each [`OwningPtr`] is a valid reference to the type represented by [`ComponentId`]
        //   (the data is store in self.raw_bytes)

        trace!(?self.temp_write_buffer.component_ids, "Inserting components into entity");
        // SAFETY: we call `buffer_insert_raw_ptrs` inside the `buffer_insert_fn` function
        unsafe { self.temp_write_buffer.batch_insert(entity_world_mut) };
        Ok(())
    }

    /// SAFETY: the ReadWordBuffer must contain bytes corresponding to the correct component type
    pub fn raw_write(
        &self,
        reader: &mut Reader,
        entity_world_mut: &mut EntityWorldMut,
        tick: Tick,
        entity_map: &mut ReceiveEntityMap,
        events: &mut ConnectionEvents,
    ) -> Result<ComponentKind, ComponentError> {
        let net_id = ComponentNetId::from_bytes(reader).map_err(SerializationError::from)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(ComponentError::NotRegistered)?;
        let replication_metadata = self
            .replication_map
            .get(kind)
            .ok_or(ComponentError::MissingReplicationFns)?;
        (replication_metadata.write)(self, reader, tick, entity_world_mut, entity_map, events)?;
        Ok(*kind)
    }

    /// Method that buffers a pointer to the component data that will be inserted
    /// in the entity inside `self.raw_bytes`
    pub fn buffer_insert<C: Component<Mutability = Mutable> + PartialEq>(
        &mut self,
        reader: &mut Reader,
        tick: Tick,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut ReceiveEntityMap,
        events: &mut ConnectionEvents,
    ) -> Result<(), ComponentError> {
        let kind = ComponentKind::of::<C>();
        let component_id = self
            .kind_to_component_id
            .get(&kind)
            .ok_or(ComponentError::NotRegistered)?;
        let component = self.raw_deserialize::<C>(reader, entity_map)?;
        let entity = entity_world_mut.id();
        debug!("Insert component {} to entity", core::any::type_name::<C>());

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
                events.push_update_component(entity, kind, tick);
                *c = component;
            }
        } else {
            // TODO: add safety comment
            unsafe {
                self.temp_write_buffer
                    .buffer_insert_raw_ptrs::<C>(component, *component_id)
            };
            // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("replication::receive::component::insert").increment(1);
                metrics::counter!(format!(
                    "replication::receive::component::{}::insert",
                    core::any::type_name::<C>()
                ))
                .increment(1);
            }
            events.push_insert_component(entity, kind, tick);
        }
        Ok(())
    }

    pub fn write<C: Component<Mutability = Mutable> + PartialEq>(
        &self,
        reader: &mut Reader,
        tick: Tick,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut ReceiveEntityMap,
        events: &mut ConnectionEvents,
    ) -> Result<(), ComponentError> {
        debug!("Writing component {} to entity", core::any::type_name::<C>());
        let kind = ComponentKind::of::<C>();
        let component = self.raw_deserialize::<C>(reader, entity_map)?;
        let entity = entity_world_mut.id();
        // TODO: should we send the event based on on the message type (Insert/Update) or based on whether the component was actually inserted?
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
                events.push_update_component(entity, kind, tick);
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
            events.push_insert_component(entity, kind, tick);
            entity_world_mut.insert(component);
        }
        Ok(())
    }

    pub fn batch_remove(
        &mut self,
        net_ids: Vec<ComponentNetId>,
        entity_world_mut: &mut EntityWorldMut,
        tick: Tick,
        events: &mut ConnectionEvents,
    ) {
        for net_id in net_ids {
            let kind = self.kind_map.kind(net_id).expect("unknown component kind");
            let replication_metadata = self
                .replication_map
                .get(kind)
                .expect("the component is not part of the protocol");
            events.push_remove_component(entity_world_mut.id(), *kind, tick);
            let remove_fn = replication_metadata
                .remove
                .expect("the component does not have a remove function");
            remove_fn(self);
        }

        entity_world_mut.remove_by_ids(&self.temp_write_buffer.component_ids);
        self.temp_write_buffer.component_ids.clear();
    }

    /// Prepare for a component being removed
    /// We don't actually remove the component here, we just push the ComponentId to the `component_ids` vector
    /// so that they can all be removed at the same time
    pub fn buffer_remove<C: Component>(&mut self) {
        let kind = ComponentKind::of::<C>();
        let component_id = self.kind_to_component_id.get(&kind).unwrap();
        self.temp_write_buffer.component_ids.push(*component_id);
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

pub fn register_component_send<C: Component>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_client {
                crate::client::replication::send::register_replicate_component_send::<C>(app);
            }
            if is_server {
                debug!(
                    "register send events on server for {}",
                    core::any::type_name::<C>()
                );
                crate::server::events::emit_replication_events::<C>(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_server {
                crate::server::replication::send::register_replicate_component_send::<C>(app);
            }
            if is_client {
                debug!(
                    "register send events on client for {}",
                    core::any::type_name::<C>()
                );
                crate::client::events::emit_replication_events::<C>(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_component_send::<C>(app, ChannelDirection::ServerToClient);
            register_component_send::<C>(app, ChannelDirection::ClientToServer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::protocol::{
        ComponentSyncModeFull, ComponentSyncModeOnce, ComponentSyncModeSimple,
    };

    #[derive(Debug, Default, Clone, PartialEq, TypePath, Resource)]
    struct Buffer(TempWriteBuffer);

    // TODO: this breaks because of https://github.com/bevyengine/bevy/pull/16219!
    /// Make sure that the temporary buffer works properly even if it's being used recursively
    /// because of observers
    #[test]
    fn test_recursive_temp_write_buffer() {
        let mut world = World::new();
        world.init_resource::<Buffer>();

        world.add_observer(
            |trigger: Trigger<OnAdd, ComponentSyncModeFull>, mut commands: Commands| {
                let entity = trigger.target();
                commands.queue(move |world: &mut World| {
                    let component_id_once = world.register_component::<ComponentSyncModeOnce>();
                    let component_id_simple = world.register_component::<ComponentSyncModeSimple>();
                    let unsafe_world = world.as_unsafe_world_cell();
                    let mut buffer = unsafe { unsafe_world.get_resource_mut::<Buffer>() }.unwrap();
                    unsafe {
                        buffer.0.buffer_insert_raw_ptrs::<_>(
                            ComponentSyncModeOnce(1.0),
                            component_id_once,
                        )
                    }
                    unsafe {
                        buffer.0.buffer_insert_raw_ptrs::<_>(
                            ComponentSyncModeSimple(1.0),
                            component_id_simple,
                        )
                    }
                    // we insert both Once and Simple into the entity
                    let mut entity_world_mut =
                        unsafe { unsafe_world.world_mut() }.entity_mut(entity);
                    // SAFETY: we call `buffer_insert_raw_ptrs` above
                    unsafe { buffer.0.batch_insert(&mut entity_world_mut) };
                })
            },
        );
        world.add_observer(
            |trigger: Trigger<OnAdd, ComponentSyncModeOnce>, mut commands: Commands| {
                let entity = trigger.target();
                commands.queue(move |world: &mut World| {
                    let component_id = world.register_component::<ComponentSyncModeSimple>();
                    let unsafe_world = world.as_unsafe_world_cell();
                    let mut buffer = unsafe { unsafe_world.get_resource_mut::<Buffer>() }.unwrap();
                    unsafe {
                        buffer
                            .0
                            .buffer_insert_raw_ptrs::<_>(ComponentSyncModeSimple(1.0), component_id)
                    }
                    // we insert only Simple into the entity.
                    // we should NOT also be inserting the components that were previously in the buffer (Once) a second time
                    let mut entity_world_mut =
                        unsafe { unsafe_world.world_mut() }.entity_mut(entity);
                    // SAFETY: we call `buffer_insert_raw_ptrs` above
                    unsafe { buffer.0.batch_insert(&mut entity_world_mut) };
                })
            },
        );
        world.add_observer(
            |trigger: Trigger<OnInsert, ComponentSyncModeSimple>,
             mut query: Query<&mut ComponentSyncModeFull>| {
                if let Ok(mut comp) = query.get_mut(trigger.target()) {
                    comp.0 += 1.0;
                }
            },
        );
        world.spawn(ComponentSyncModeFull(0.0));
        world.flush();

        // make sure that the ComponentSyncModeSimple was only inserted twice, not three times
        assert_eq!(
            world
                .query::<&ComponentSyncModeFull>()
                .single(&world)
                .unwrap()
                .0,
            2.0
        );
    }
}
