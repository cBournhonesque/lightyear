use crate::prelude::{
    AppMessageExt, ChannelDirection, ComponentRegistry, Message, MessageRegistry,
    ReplicateResourceMetadata,
};
use crate::protocol::message::MessageType;
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::reader::BitcodeReader;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::replication::entity_map::EntityMap;
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::Resource;
use bitcode::__private::Fixed;
use std::any::TypeId;

// TODO: maybe instead of MessageFns, use an erased trait objects? like dyn ErasedSerialize + ErasedDeserialize ?
//  but how do we deal with implementing behaviour for types that don't have those traits?
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedSerializeFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub serialize: unsafe fn(),
    pub deserialize: unsafe fn(),
    pub map_entities: Option<unsafe fn()>,
}

pub struct SerializeFns<M> {
    pub serialize: SerializeFn<M>,
    pub deserialize: DeserializeFn<M>,
    pub map_entities: Option<MapEntitiesFn<M>>,
}

type SerializeFn<M> = fn(&M, writer: &mut BitcodeWriter) -> anyhow::Result<()>;
type DeserializeFn<M> = fn(reader: &mut BitcodeReader) -> anyhow::Result<M>;
pub(crate) type MapEntitiesFn<M> = fn(&mut M, entity_map: &mut EntityMap);

impl ErasedSerializeFns {
    pub(crate) fn new<M: Message>() -> Self {
        let serialize: SerializeFn<M> = <M as BitSerializable>::encode;
        let deserialize: DeserializeFn<M> = <M as BitSerializable>::decode;
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            serialize: unsafe { std::mem::transmute(serialize) },
            deserialize: unsafe { std::mem::transmute(deserialize) },
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
            deserialize: unsafe { std::mem::transmute(self.deserialize) },
            map_entities: self.map_entities.map(|m| unsafe { std::mem::transmute(m) }),
        }
    }

    pub(crate) fn add_map_entities<M: MapEntities + 'static>(&mut self) {
        let map_entities: MapEntitiesFn<M> = <M as MapEntities>::map_entities::<EntityMap>;
        self.map_entities = Some(unsafe { std::mem::transmute(map_entities) });
    }

    pub(crate) fn map_entities<M: 'static>(&self, message: &mut M, entity_map: &mut EntityMap) {
        let fns = unsafe { self.typed::<M>() };
        if let Some(map_entities_fn) = fns.map_entities {
            map_entities_fn(message, entity_map)
        }
    }

    pub(crate) fn serialize<M: 'static>(
        &self,
        message: &M,
        writer: &mut BitcodeWriter,
    ) -> anyhow::Result<()> {
        let fns = unsafe { self.typed::<M>() };
        (fns.serialize)(message, writer)
    }

    /// Deserialize the message value from the reader
    pub(crate) fn deserialize<M: 'static>(
        &self,
        reader: &mut BitcodeReader,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<M> {
        let fns = unsafe { self.typed::<M>() };
        let mut message = (fns.deserialize)(reader)?;
        if let Some(map_entities) = fns.map_entities {
            map_entities(&mut message, entity_map);
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
        let mut registry = self.world.resource_mut::<MessageRegistry>();
        registry.try_add_map_entities::<M>();
        let mut registry = self.world.resource_mut::<ComponentRegistry>();
        registry.try_add_map_entities::<M>();
    }
}
