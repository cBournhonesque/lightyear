use anyhow::Context;
use bevy::app::PreUpdate;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::Debug;

use crate::_reexport::{ReadBuffer, ReadWordBuffer, WriteBuffer, WriteWordBuffer};
use crate::client::message::add_server_to_client_message;
use crate::prelude::{client, server, Channel, RemoteEntityMap};
use bevy::prelude::{
    App, EntityMapper, EventWriter, IntoSystemConfigs, ResMut, Resource, TypePath, World,
};
use bevy::utils::HashMap;
use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::error;

use crate::inputs::native::input_buffer::InputMessage;
use crate::packet::message::Message;
use crate::prelude::{ChannelDirection, ChannelKind, MainSet};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::{BitSerializable, EventContext, Protocol};
use crate::server::message::add_client_to_server_message;
use crate::shared::replication::entity_map::EntityMap;

pub enum InputMessageKind {
    /// This is a message for a [`LeafwingUserAction`](crate::inputs::leafwing::LeafwingUserAction)
    #[cfg(feature = "leafwing")]
    Leafwing,
    /// This is a message for a [`UserAction`](crate::inputs::native::UserAction)
    Native,
    /// This is not an input message, but a regular [`Message`]
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ErasedMessageFns {
    type_id: TypeId,
    type_name: &'static str,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub serialize: unsafe fn(),
    pub deserialize: unsafe fn(),
    pub map_entities: Option<unsafe fn()>,
    pub message_type: MessageType,
}

type SerializeFn<M> = fn(&M, writer: &mut WriteWordBuffer) -> anyhow::Result<()>;
type DeserializeFn<M> = fn(reader: &mut ReadWordBuffer) -> anyhow::Result<M>;
type MapEntitiesFn<M> = fn(&mut M, entity_map: &mut EntityMap);

pub struct MessageFns<M> {
    pub serialize: SerializeFn<M>,
    pub deserialize: DeserializeFn<M>,
    pub map_entities: Option<MapEntitiesFn<M>>,
    pub message_type: MessageType,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MessageType {
    /// This is a message for a [`LeafwingUserAction`](crate::inputs::leafwing::LeafwingUserAction)
    #[cfg(feature = "leafwing")]
    LeafwingInput,
    /// This is a message for a [`UserAction`](crate::inputs::native::UserAction)
    NativeInput,
    /// This is not an input message, but a regular [`Message`]
    Normal,
}

impl ErasedMessageFns {
    pub(crate) unsafe fn typed<M: Message>(&self) -> MessageFns<M> {
        debug_assert_eq!(
            self.type_id,
            TypeId::of::<M>(),
            "The erased message fns were created for type {}, but we are trying to convert to type {}",
            self.type_name,
            std::any::type_name::<M>(),
        );

        MessageFns {
            serialize: unsafe { std::mem::transmute(self.serialize) },
            deserialize: unsafe { std::mem::transmute(self.deserialize) },
            map_entities: self.map_entities.map(|m| unsafe { std::mem::transmute(m) }),
            message_type: self.message_type,
        }
    }
}

#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct MessageRegistry {
    // TODO: maybe instead of MessageFns, use an erased trait objects? like dyn ErasedSerialize + ErasedDeserialize ?
    //  but how do we deal with implementing behaviour for types that don't have those traits?
    fns_map: HashMap<MessageKind, ErasedMessageFns>,
    pub(crate) kind_map: TypeMapper<MessageKind>,
}

/// Add a message to the list of messages that can be sent
pub trait AppMessageExt {
    fn add_message<M: Message>(&mut self, direction: ChannelDirection);

    fn add_message_mapped<M: Message + MapEntities>(&mut self, direction: ChannelDirection);
}

fn register_message_send<M: Message>(app: &mut App, direction: ChannelDirection) {
    match direction {
        ChannelDirection::ClientToServer => {
            add_client_to_server_message::<M>(app);
        }
        ChannelDirection::ServerToClient => {
            add_server_to_client_message::<M>(app);
        }
        ChannelDirection::Bidirectional => {
            register_message_send::<M>(app, ChannelDirection::ClientToServer);
            register_message_send::<M>(app, ChannelDirection::ServerToClient);
        }
    }
}

impl AppMessageExt for App {
    fn add_message<M: Message>(&mut self, direction: ChannelDirection) {
        if let Some(mut protocol) = self.world.get_resource_mut::<MessageRegistry>() {
            protocol.add_message::<M>(MessageType::Normal);
        } else {
            todo!("create a protocol");
        }
        register_message_send::<M>(self, direction);
    }

    fn add_message_mapped<M: Message + MapEntities>(&mut self, direction: ChannelDirection) {
        if let Some(mut protocol) = self.world.get_resource_mut::<MessageRegistry>() {
            protocol.add_message_mapped::<M>(MessageType::Normal);
        } else {
            todo!("create a protocol");
        }
        register_message_send::<M>(self, direction);
    }
}

impl MessageRegistry {
    pub(crate) fn message_type(&self, net_id: NetId) -> MessageType {
        let kind = self.kind_map.kind(net_id).unwrap();
        self.fns_map
            .get(kind)
            .map(|fns| fns.message_type)
            .unwrap_or(MessageType::Normal)
    }
    pub(crate) fn add_message<M: Message>(&mut self, message_type: MessageType) {
        let message_kind = self.kind_map.add::<M>();
        let serialize: SerializeFn<M> = <M as BitSerializable>::encode;
        let deserialize: DeserializeFn<M> = <M as BitSerializable>::decode;
        self.fns_map.insert(
            message_kind,
            ErasedMessageFns {
                type_id: TypeId::of::<M>(),
                type_name: std::any::type_name::<M>(),
                serialize: unsafe { std::mem::transmute(serialize) },
                deserialize: unsafe { std::mem::transmute(deserialize) },
                map_entities: None,
                message_type,
            },
        );
    }
    pub(crate) fn add_message_mapped<M: Message + MapEntities>(
        &mut self,
        message_type: MessageType,
    ) {
        let message_kind = self.kind_map.add::<M>();
        let serialize: SerializeFn<M> = <M as BitSerializable>::encode;
        let deserialize: DeserializeFn<M> = <M as BitSerializable>::decode;
        let map_entities: MapEntitiesFn<M> = <M as MapEntities>::map_entities::<EntityMap>;
        self.fns_map.insert(
            message_kind,
            ErasedMessageFns {
                type_id: TypeId::of::<M>(),
                type_name: std::any::type_name::<M>(),
                serialize: unsafe { std::mem::transmute(serialize) },
                deserialize: unsafe { std::mem::transmute(deserialize) },
                map_entities: Some(unsafe { std::mem::transmute(map_entities) }),
                message_type,
            },
        );
    }

    pub(crate) fn serialize<M: Message>(
        &self,
        message: &M,
        writer: &mut WriteWordBuffer,
    ) -> anyhow::Result<()> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the message is not part of the protocol")?;
        let fns = unsafe { erased_fns.typed::<M>() };
        let net_id = self.kind_map.net_id(&kind).unwrap();
        writer.encode(net_id, Fixed)?;
        (fns.serialize)(message, writer)
    }

    pub(crate) fn deserialize<M: Message>(
        &self,
        reader: &mut ReadWordBuffer,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<M> {
        let net_id = reader.decode::<NetId>(Fixed)?;
        let kind = self.kind_map.kind(net_id).context("unknown message kind")?;
        let erased_fns = self
            .fns_map
            .get(kind)
            .context("the message is not part of the protocol")?;
        let fns = unsafe { erased_fns.typed::<M>() };
        let mut message = (fns.deserialize)(reader)?;
        if let Some(map_entities) = fns.map_entities {
            map_entities(&mut message, entity_map);
        }
        Ok(message)
    }

    pub(crate) fn map_entities<M: Message>(&self, message: &mut M, entity_map: &mut EntityMap) {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .fns_map
            .get(&kind)
            .context("the message is not part of the protocol")
            .unwrap();
        let fns = unsafe { erased_fns.typed::<M>() };
        if let Some(map_entities) = fns.map_entities {
            map_entities(message, entity_map);
        }
    }
}

/// A [`MessageProtocol`] is basically an enum that contains all the [`Message`] that can be sent
/// over the network.
pub trait MessageProtocol:
    BitSerializable
    + Serialize
    + DeserializeOwned
    + Clone
    + MapEntities
    + Debug
    + Send
    + Sync
    + From<InputMessage<<<Self as MessageProtocol>::Protocol as Protocol>::Input>>
    + TryInto<InputMessage<<<Self as MessageProtocol>::Protocol as Protocol>::Input>, Error = ()>
{
    type Protocol: Protocol;

    /// Get the name of the Message
    fn name(&self) -> &'static str;

    /// Returns the MessageKind of the Message
    fn kind(&self) -> MessageKind;

    /// Returns true if the message is an input message
    fn input_message_kind(&self) -> InputMessageKind;

    // TODO: combine these 2 into a single function that takes app?
    /// Add events to the app
    fn add_events<Ctx: EventContext>(app: &mut App);
}

/// [`MessageKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
    pub fn of<M: Message>() -> Self {
        Self(TypeId::of::<M>())
    }
}

impl TypeKind for MessageKind {}

impl From<TypeId> for MessageKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}
