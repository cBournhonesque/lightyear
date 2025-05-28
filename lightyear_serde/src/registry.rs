use crate::entity_map::{EntityMap, ReceiveEntityMap, SendEntityMap};
use crate::reader::Reader;
use crate::writer::Writer;
use crate::{SerializationError, ToBytes};
use bevy::ecs::entity::MapEntities;
use bevy::ptr::{Ptr, PtrMut};
use core::any::TypeId;
use serde::de::DeserializeOwned;
use serde::Serialize;

// TODO: this should be in lightyear_serde? it's not strictly related to messages?
/// Stores function pointers related to serialization and deserialization
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedSerializeFns {
    pub(crate) type_id: TypeId,
    pub type_name: &'static str,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub erased_serialize: ErasedSerializeFn,
    pub serialize: unsafe fn(),
    pub context_serialize: unsafe fn(),
    pub deserialize: unsafe fn(),
    pub context_deserialize: unsafe fn(),
    pub erased_clone: Option<unsafe fn()>,
    pub map_entities: Option<ErasedMapEntitiesFn>,
}

pub struct ContextSerializeFns<C, M, I = M> {
    /// Called to serialize the type into the writer
    pub serialize: SerializeFn<I>,
    pub context_serialize: ContextSerializeFn<C, M, I>,
}

impl<C, M> ContextSerializeFns<C, M, M> {
    pub fn new(serialize: SerializeFn<M>) -> Self {
        Self {
            serialize,
            context_serialize: default_context_serialize::<C, M>,
        }
    }
}

impl<C, M, I> ContextSerializeFns<C, M, I> {
    pub fn with_context<M2>(
        self,
        context_serialize: ContextSerializeFn<C, M2, I>,
    ) -> ContextSerializeFns<C, M2, I> {
        ContextSerializeFns {
            context_serialize,
            serialize: self.serialize,
        }
    }
    pub fn serialize(
        self,
        context: &mut C,
        message: &M,
        writer: &mut Writer,
    ) -> Result<(), SerializationError> {
        (self.context_serialize)(context, message, writer, self.serialize)
    }
}

pub struct ContextDeserializeFns<C, M, I = M> {
    /// Called to deserialize the type from the reader
    pub deserialize: DeserializeFn<I>,
    pub context_deserialize: ContextDeserializeFn<C, M, I>,
}

impl<C, M> ContextDeserializeFns<C, M, M> {
    pub fn new(deserialize: DeserializeFn<M>) -> Self {
        Self {
            deserialize,
            context_deserialize: default_context_deserialize::<C, M>,
        }
    }
}

impl<C, M, I> ContextDeserializeFns<C, M, I> {
    pub fn with_context<M2>(
        self,
        context_deserialize: ContextDeserializeFn<C, M2, I>,
    ) -> ContextDeserializeFns<C, M2, I> {
        ContextDeserializeFns {
            context_deserialize,
            deserialize: self.deserialize,
        }
    }
    pub fn deserialize(
        self,
        context: &mut C,
        reader: &mut Reader,
    ) -> Result<M, SerializationError> {
        (self.context_deserialize)(context, reader, self.deserialize)
    }
}

/// Controls how a type (resources/components/messages) is serialized and deserialized
pub struct SerializeFns<M> {
    /// Called to serialize the type into the writer
    pub serialize: SerializeFn<M>,
    /// Called to deserialize the type from the reader
    pub deserialize: DeserializeFn<M>,
}

impl<M: Serialize + DeserializeOwned> Default for SerializeFns<M> {
    fn default() -> Self {
        Self {
            serialize: default_serialize::<M>,
            deserialize: default_deserialize::<M>,
        }
    }
}

impl<M: ToBytes> SerializeFns<M> {
    pub fn with_to_bytes() -> Self {
        Self {
            serialize: |message, writer| message.to_bytes(writer),
            deserialize: |reader| M::from_bytes(reader),
        }
    }
}

type ErasedSerializeFn = unsafe fn(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: &mut SendEntityMap,
) -> Result<(), SerializationError>;

/// Type of the serialize function without entity mapping
pub type SerializeFn<M> = fn(message: &M, writer: &mut Writer) -> Result<(), SerializationError>;

/// Type of the deserialize function without entity mapping
pub type DeserializeFn<M> = fn(reader: &mut Reader) -> Result<M, SerializationError>;

#[doc(hidden)]
/// Type of the serialize function with entity mapping
pub type ContextSerializeFn<C, M, I> =
    fn(&mut C, message: &M, writer: &mut Writer, SerializeFn<I>) -> Result<(), SerializationError>;

#[doc(hidden)]
/// Type of the deserialize function with entity mapping
pub type ContextDeserializeFn<C, M, I> =
    fn(&mut C, reader: &mut Reader, DeserializeFn<I>) -> Result<M, SerializationError>;

#[allow(unused)]
type CloneFn<M> = fn(&M) -> M;

/// Type of the entity mapping function
pub(crate) type ErasedMapEntitiesFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut EntityMap);

fn default_context_serialize<C, M>(
    _: &mut C,
    message: &M,
    writer: &mut Writer,
    serialize_fn: SerializeFn<M>,
) -> Result<(), SerializationError> {
    serialize_fn(message, writer)
}

fn default_context_deserialize<C, M>(
    _: &mut C,
    reader: &mut Reader,
    deserialize_fn: DeserializeFn<M>,
) -> Result<M, SerializationError> {
    deserialize_fn(reader)
}

#[cfg(feature = "std")]
/// Default serialize function using bincode
fn default_serialize<M: Serialize>(
    message: &M,
    buffer: &mut Writer,
) -> Result<(), SerializationError> {
    let _ = bincode::serde::encode_into_std_write(message, buffer, bincode::config::standard())?;
    Ok(())
}

#[cfg(not(feature = "std"))]
/// Default serialize function using bincode
fn default_serialize<M: Serialize>(
    message: &M,
    buffer: &mut Writer,
) -> Result<(), SerializationError> {
    bincode::serde::encode_into_writer(message, buffer, bincode::config::standard())?;
    Ok(())
}

#[cfg(feature = "std")]
/// Default deserialize function using bincode
fn default_deserialize<M: DeserializeOwned>(buffer: &mut Reader) -> Result<M, SerializationError> {
    let data = bincode::serde::decode_from_std_read(buffer, bincode::config::standard())?;
    Ok(data)
}

#[cfg(not(feature = "std"))]
/// Default deserialize function using bincode
fn default_deserialize<M: DeserializeOwned>(buffer: &mut Reader) -> Result<M, SerializationError> {
    let data = bincode::serde::decode_from_reader(buffer, bincode::config::standard())?;
    Ok(data)
}

fn erased_clone<M: Clone>(message: &M) -> M {
    message.clone()
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut EntityMap,
) {
    // SAFETY: the PtrMut must be a valid pointer to a value of type M
    let message = unsafe { message.deref_mut::<M>() };
    M::map_entities(message, entity_map);
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_send_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut SendEntityMap,
) {
    // SAFETY: the PtrMut must be a valid pointer to a value of type M
    let message = unsafe { message.deref_mut::<M>() };
    M::map_entities(message, entity_map);
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_receive_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut ReceiveEntityMap,
) {
    // SAFETY: the PtrMut must be a valid pointer to a value of type M
    let message = unsafe { message.deref_mut::<M>() };
    M::map_entities(message, entity_map);
}

unsafe fn erased_serialize_fn<M: 'static>(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: &mut SendEntityMap,
) -> Result<(), SerializationError> {
    unsafe {
        // SAFETY: the Ptr was created for the message of type M
        let message = message.deref::<M>();
        erased_serialize_fn.serialize::<_, M, M>(message, writer, entity_map)
    }
}

impl ErasedSerializeFns {
    pub fn new<SerContext, DeContext, M: 'static, I: 'static>(
        serialize: ContextSerializeFns<SerContext, M, I>,
        deserialize: ContextDeserializeFns<DeContext, M, I>,
    ) -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            type_name: core::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { core::mem::transmute(serialize.serialize) },
            context_serialize: unsafe { core::mem::transmute(serialize.context_serialize) },
            deserialize: unsafe { core::mem::transmute(deserialize.deserialize) },
            context_deserialize: unsafe { core::mem::transmute(deserialize.context_deserialize) },
            erased_clone: None,
            map_entities: None,
        }
    }

    // We need to be able to clone the data, because when serialize we:
    // - clone the data
    // - map the entities
    // - serialize the cloned data
    // Note that this is fairly inefficient because in most cases (when there is no authority transfer)
    // there is no entity mapping done on the serialization side, just on the deserialization side.
    // However, components that contain other entities should be small in general.
    pub fn add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        self.map_entities = Some(erased_map_entities::<M>);
        let clone_fn: fn(&M) -> M = erased_clone::<M>;
        self.erased_clone = Some(unsafe { core::mem::transmute(clone_fn) });
    }

    pub fn map_entities<M: 'static>(&self, message: &mut M, entity_map: &mut EntityMap) {
        let ptr = PtrMut::from(message);
        if let Some(map_entities_fn) = self.map_entities {
            unsafe {
                map_entities_fn(ptr, entity_map);
            }
        }
    }

    /// Serialize the message into the writer.
    /// If available, we try to map the entities in the message from local to remote.
    ///
    /// # Safety
    /// the ErasedSerializeFns must be created for the type M
    pub unsafe fn serialize<C, M: 'static, I>(
        &self,
        message: &M,
        writer: &mut Writer,
        context: &mut C,
    ) -> Result<(), SerializationError> {
        let serialize: SerializeFn<I> = unsafe { core::mem::transmute(self.serialize) };
        let context_serialize: ContextSerializeFn<C, M, I> =
            unsafe { core::mem::transmute(self.context_serialize) };
        context_serialize(context, message, writer, serialize)
    }

    /// Deserialize the message value from the reader
    ///
    /// # Safety
    /// the ErasedSerializeFns must be created for the type M
    pub unsafe fn deserialize<C, M: 'static, I>(
        &self,
        reader: &mut Reader,
        context: &mut C,
    ) -> Result<M, SerializationError> {
        let deserialize: DeserializeFn<I> = unsafe { core::mem::transmute(self.deserialize) };
        let context_deserialize: ContextDeserializeFn<C, M, I> =
            unsafe { core::mem::transmute(self.context_deserialize) };
        context_deserialize(context, reader, deserialize)
    }
    
    /// Get the deserialize functions for the type M.
    ///
    /// # Safety
    /// the ErasedSerializeFns must be created for the type M
    pub unsafe fn deserialize_fns<C, M: 'static, I>(&self) -> ContextDeserializeFns<C, M, I> {
        let deserialize: DeserializeFn<I> = unsafe { core::mem::transmute(self.deserialize) };
        let context_deserialize: ContextDeserializeFn<C, M, I> =
            unsafe { core::mem::transmute(self.context_deserialize) };
        ContextDeserializeFns {
            deserialize,
            context_deserialize
        }
    }
}
