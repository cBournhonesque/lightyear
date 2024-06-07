use crate::prelude::{ComponentRegistry, Message, MessageRegistry};
use crate::serialize::{reader::Reader, writer::Writer, SerializationError};
use crate::shared::replication::entity_map::EntityMap;
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::ptr::{Ptr, PtrMut};
use std::any::TypeId;

// TODO: maybe instead of MessageFns, use an erased trait objects? like dyn ErasedSerialize + ErasedDeserialize ?
//  but how do we deal with implementing behaviour for types that don't have those traits?
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedSerializeFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub serialize: ErasedSerializeFn,
    pub deserialize: unsafe fn(),
    pub map_entities: Option<ErasedMapEntitiesFn>,
}

pub struct SerializeFns<M> {
    pub deserialize: DeserializeFn<M>,
}

type ErasedSerializeFn =
    unsafe fn(message: Ptr, writer: &mut Writer) -> Result<(), SerializationError>;
type DeserializeFn<M> = fn(reader: &mut Reader) -> Result<M, SerializationError>;

pub(crate) type ErasedMapEntitiesFn = unsafe fn(message: PtrMut, entity_map: &mut EntityMap);

/// SAFETY: the Ptr must be a valid pointer to a value of type M
unsafe fn erased_serialize<M: Message>(
    message: Ptr,
    buffer: &mut Writer,
) -> Result<(), SerializationError> {
    let data = message.deref::<M>();
    let _ = bincode::serde::encode_into_std_write(data, buffer, bincode::config::standard())?;
    Ok(())
}

fn erased_deserialize<M: Message>(buffer: &mut Reader) -> Result<M, SerializationError> {
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
    pub(crate) fn new<M: Message>() -> Self {
        let erased_deserialize: DeserializeFn<M> = erased_deserialize::<M>;
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            serialize: erased_serialize::<M>,
            deserialize: unsafe { std::mem::transmute(erased_deserialize) },
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
            deserialize: unsafe { std::mem::transmute(self.deserialize) },
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

    /// SAFETY: the ErasedSerializeFns must be created for the type M
    pub(crate) unsafe fn serialize<M: 'static>(
        &self,
        message: &M,
        writer: &mut Writer,
    ) -> Result<(), SerializationError> {
        let ptr = Ptr::from(message);
        (self.serialize)(ptr, writer)
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
