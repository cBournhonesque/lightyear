use crate::prelude::{ComponentRegistry, Message, MessageRegistry};
use crate::serialize::{reader::Reader, writer::Writer, SerializationError};
use crate::shared::replication::entity_map::EntityMap;
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::ptr::{Ptr, PtrMut};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::any::TypeId;

/// Stores function pointers related to serialization and deserialization
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedSerializeFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub serialize: unsafe fn(),
    pub erased_serialize: ErasedSerializeFn,
    pub deserialize: unsafe fn(),
    pub map_entities: Option<ErasedMapEntitiesFn>,
}

pub struct SerializeFns<M> {
    pub serialize: SerializeFn<M>,
    pub deserialize: DeserializeFn<M>,
}

type ErasedSerializeFn = unsafe fn(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
) -> Result<(), SerializationError>;
type SerializeFn<M> = fn(message: &M, writer: &mut Writer) -> Result<(), SerializationError>;
type DeserializeFn<M> = fn(reader: &mut Reader) -> Result<M, SerializationError>;

pub(crate) type ErasedMapEntitiesFn = unsafe fn(message: PtrMut, entity_map: &mut EntityMap);

unsafe fn erased_serialize_fn<M: Message>(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
) -> Result<(), SerializationError> {
    let typed_serialize_fns = erased_serialize_fn.typed::<M>();
    let message = message.deref::<M>();
    (typed_serialize_fns.serialize)(message, writer)
}

/// Default serialize function using bincode
fn default_serialize<M: Message + Serialize>(
    message: &M,
    buffer: &mut Writer,
) -> Result<(), SerializationError> {
    let _ = bincode::serde::encode_into_std_write(message, buffer, bincode::config::standard())?;
    Ok(())
}

/// Default deserialize function using bincode
fn default_deserialize<M: Message + DeserializeOwned>(
    buffer: &mut Reader,
) -> Result<M, SerializationError> {
    let data = bincode::serde::decode_from_std_read(buffer, bincode::config::standard())?;
    Ok(data)
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut EntityMap,
) {
    let data = message.deref_mut::<M>();
    M::map_entities(data, entity_map);
}

impl ErasedSerializeFns {
    pub(crate) fn new<M: Message + Serialize + DeserializeOwned>() -> Self {
        let serialize_fns = SerializeFns {
            serialize: default_serialize::<M>,
            deserialize: default_deserialize::<M>,
        };
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { std::mem::transmute(serialize_fns.serialize) },
            deserialize: unsafe {
                std::mem::transmute::<
                    for<'a> fn(&'a mut Reader) -> std::result::Result<M, SerializationError>,
                    unsafe fn(),
                >(serialize_fns.deserialize)
            },
            map_entities: None,
        }
    }

    pub(crate) fn new_custom_serde<M: Message>(serialize_fns: SerializeFns<M>) -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { std::mem::transmute(serialize_fns.serialize) },
            deserialize: unsafe { std::mem::transmute(serialize_fns.deserialize) },
            map_entities: None,
        }
    }

    pub(crate) unsafe fn typed<M: 'static>(&self) -> SerializeFns<M> {
        debug_assert_eq!(
            self.type_id,
            TypeId::of::<M>(),
            "The erased message fns were created for type {}, but we are trying to convert to type {}",
            self.type_name,
            std::any::type_name::<M>(),
        );
        SerializeFns {
            serialize: unsafe { std::mem::transmute(self.serialize) },
            deserialize: unsafe {
                std::mem::transmute::<
                    unsafe fn(),
                    for<'a> fn(&'a mut Reader) -> std::result::Result<M, SerializationError>,
                >(self.deserialize)
            },
        }
    }

    pub(crate) fn add_map_entities<M: MapEntities + 'static>(&mut self) {
        self.map_entities = Some(erased_map_entities::<M>);
    }

    pub(crate) fn map_entities<M: 'static>(&self, message: &mut M, entity_map: &mut EntityMap) {
        let ptr = PtrMut::from(message);
        if let Some(map_entities_fn) = self.map_entities {
            unsafe { map_entities_fn(ptr, entity_map) }
        }
    }

    /// SAFETY: the ErasedSerializeFns must be created for the type of the Ptr
    pub(crate) unsafe fn erased_serialize(
        &self,
        message: Ptr,
        writer: &mut Writer,
    ) -> Result<(), SerializationError> {
        (self.erased_serialize)(self, message, writer)
    }

    /// SAFETY: the ErasedSerializeFns must be created for the type M
    pub(crate) unsafe fn serialize<M: 'static>(
        &self,
        message: &M,
        writer: &mut Writer,
    ) -> Result<(), SerializationError> {
        let fns = unsafe { self.typed::<M>() };
        (fns.serialize)(message, writer)
    }

    /// Deserialize the message value from the reader
    ///
    /// SAFETY: the ErasedSerializeFns must be created for the type M
    pub(crate) unsafe fn deserialize<M: 'static>(
        &self,
        reader: &mut Reader,
        entity_map: &mut EntityMap,
    ) -> Result<M, SerializationError> {
        let fns = unsafe { self.typed::<M>() };
        let mut message = (fns.deserialize)(reader)?;
        if let Some(map_entities) = self.map_entities {
            map_entities(PtrMut::from(&mut message), entity_map);
        }
        Ok(message)
    }
}

pub trait AppSerializeExt {
    /// Indicate that the type `M` contains Entity references, and that the entities
    /// should be mapped during deserialization
    fn add_map_entities<M: MapEntities + 'static>(&mut self);
}

impl AppSerializeExt for App {
    // TODO: should we return Result<()> to indicate if adding the map_entities was successful?
    //  otherwise it might not work if the message was not registered before
    // TODO: or have a single SerializeRegistry?
    // TODO: or should we just have add_message_map_entities and add_component_map_entities?
    fn add_map_entities<M: MapEntities + 'static>(&mut self) {
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        registry.try_add_map_entities::<M>();
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.try_add_map_entities::<M>();
    }
}
