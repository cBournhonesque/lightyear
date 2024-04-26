use anyhow::Context;
use bevy::app::PreUpdate;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::Debug;

use crate::_reexport::{ReadBuffer, ReadWordBuffer, WriteBuffer, WriteWordBuffer};
use crate::client::message::add_server_to_client_message;
use crate::prelude::{client, server};
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
#[cfg(feature = "leafwing")]
use crate::shared::events::components::InputMessageEvent;
use crate::shared::events::connection::IterMessageEvent;

// client writes an Enum containing all their message type
// each message must derive message

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
    // pub map_entities: Option<unsafe fn()>,
    pub message_type: MessageType,
}

type SerializeFn<M> = fn(&M, writer: &mut WriteWordBuffer) -> anyhow::Result<()>;
type DeserializeFn<M> = fn(reader: &mut ReadWordBuffer) -> anyhow::Result<M>;

pub struct MessageFns<M> {
    pub serialize: SerializeFn<M>,
    pub deserialize: DeserializeFn<M>,
    // TODO: how to handle map entities, since map_entities takes a generic arg?
    // pub map_entities: Option<fn<M: EntityMapper>(&mut self, entity_mapper: &mut M);>,
    pub message_type: MessageType,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MessageType {
    #[cfg(feature = "leafwing")]
    LeafwingInput,
    NativeInput,
    Normal,
}

impl ErasedMessageFns {
    unsafe fn typed<M: Message>(&self) -> MessageFns<M> {
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

fn default_serialize<M: Message>(message: &M, writer: &mut WriteWordBuffer) -> anyhow::Result<()> {
    message.encode(writer)
}

/// Add a message to the list of messages that can be sent
pub trait AppMessageExt {
    fn add_message<M: Message>(&mut self, direction: ChannelDirection);
}

impl AppMessageExt for App {
    fn add_message<M: Message>(&mut self, direction: ChannelDirection) {
        if let Some(mut protocol) = self.world.get_resource_mut::<MessageRegistry>() {
            protocol.add_message::<M>(MessageType::Normal);
        } else {
            todo!("create a protocol");
        }
        match direction {
            ChannelDirection::ClientToServer => {
                add_client_to_server_message::<M>(self);
            }
            ChannelDirection::ServerToClient => {
                add_server_to_client_message::<M>(self);
            }
            ChannelDirection::Bidirectional => {
                add_client_to_server_message::<M>(self);
                add_server_to_client_message::<M>(self);
            }
        }
    }
}

// #[derive(Encode, Decode, Clone)]
// pub struct WithNetId<'a, M: Message> {
//     pub net_id: NetId,
//     #[bitcode(with_serde)]
//     pub data: &'a M,
// }
//
// impl<M: Message> BitSerializable for WithNetId<'_, M> {
//     fn encode(&self, writer: &mut WriteWordBuffer) -> anyhow::Result<()> {
//         self.net_id.encode(writer, Fixed)?;
//         self.data.encode(writer)
//     }
//
//     fn decode(reader: &mut ReadWordBuffer) -> anyhow::Result<Self>
//     where
//         Self: Sized,
//     {
//         let net_id = reader.decode::<NetId>(Fixed)?;
//         let data = M::decode(reader)?;
//         Ok(Self { net_id, data })
//     }
// }

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
                // map_entities: None,
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
        error!("in serialize: {:?}, net_id: {:?}", self, net_id);
        writer.encode(net_id, Fixed)?;
        (fns.serialize)(message, writer)
    }

    pub(crate) fn deserialize<M: Message>(&self, reader: &mut ReadWordBuffer) -> anyhow::Result<M> {
        let net_id = reader.decode::<NetId>(Fixed)?;
        let kind = self.kind_map.kind(net_id).context("unknown message kind")?;
        let erased_fns = self
            .fns_map
            .get(kind)
            .context("the message is not part of the protocol")?;
        let fns = unsafe { erased_fns.typed::<M>() };
        (fns.deserialize)(reader)
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

    /// Takes messages that were written and writes MessageEvents
    fn push_message_events<E: IterMessageEvent<Self::Protocol, Ctx>, Ctx: EventContext>(
        world: &mut World,
        events: &mut E,
    );
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
