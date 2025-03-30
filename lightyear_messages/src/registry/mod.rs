use crate::receive::{MessageReceiver, ReceiveMessageFn};
use crate::registry::serialize::{ErasedSerializeFns, SerializeFns};
use crate::send::{MessageSender, SendMessageFn};
use crate::{Message, MessageId};
use bevy::ecs::component::ComponentId;
use bevy::ecs::entity::MapEntities;
use bevy::platform_support::collections::HashMap;
use bevy::prelude::*;
use core::any::TypeId;
use lightyear_core::network::NetId;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::Writer;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::senders::ChannelSenderEnum;
use lightyear_transport::channel::{Channel, ChannelKind};
use lightyear_transport::entity_map::{ReceiveEntityMap, SendEntityMap};
use lightyear_transport::prelude::{ChannelMode, ChannelRegistry, ChannelSettings};
use lightyear_utils::registry::{TypeKind, TypeMapper};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::debug;

pub(crate) mod serialize;


#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    #[error("the message if of the wrong type")]
    IncorrectType,
    #[error("message is not registered in the protocol")]
    NotRegistered,
    #[error("missing serialization functions for message")]
    MissingSerializationFns,
    #[error(transparent)]
    Serialization(#[from] lightyear_serde::SerializationError),
    #[error(transparent)]
    Packet(#[from] lightyear_transport::packet::error::PacketError),
    #[error("the component id {0:?} is missing from the entity")]
    MissingComponent(ComponentId),
    #[error("the channel kind {0:?} is missing from the entity")]
    MissingChannelKind(ChannelKind),
    #[error("the message kind {0:?} is not registered")]
    UnrecognizedMessage(MessageKind),
    #[error("the message id {0:?} is not registered")]
    UnrecognizedMessageId(MessageId),
    #[error(transparent)]
    TransportError(#[from] lightyear_transport::error::TransportError),
}

/// [`MessageKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
    #[inline(always)]
    pub fn of<M: 'static>() -> Self {
        Self(TypeId::of::<M>())
    }
}

impl TypeKind for MessageKind {}

impl From<TypeId> for MessageKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}



#[derive(Debug, Clone, PartialEq)]
pub struct ReceiveMessageMetadata {
    /// ComponentId of the MessageReceiver<M> component
    pub(crate) component_id: ComponentId,
    pub(crate) receive_message_fn: ReceiveMessageFn,
}



#[derive(Debug, Clone, PartialEq, TypePath)]
pub(crate) struct SendMessageMetadata {
    /// ComponentId of the MessageSender<M> component
    pub(crate) component_id: ComponentId,
    pub(crate) send_message_fn: SendMessageFn,
}

/// A [`Resource`] that will keep track of all the [`Message`]s that can be sent over the network.
/// A [`Message`] is any type that is serializable and deserializable.
///
///
/// ### Adding Messages
///
/// You register messages by calling the [`add_message`](AppMessageExt::register_message) method directly on the App.
/// You can provide a [`ChannelDirection`] to specify if the message should be sent from the client to the server, from the server to the client, or both.
///
/// ```rust
/// use bevy::prelude::*;
/// use serde::{Deserialize, Serialize};
/// use lightyear::prelude::*;
///
/// #[derive(Serialize, Deserialize)]
/// struct MyMessage;
///
/// fn add_messages(app: &mut App) {
///   app.register_message::<MyMessage>(ChannelDirection::Bidirectional);
/// }
/// ```
///
/// ### Customizing Message behaviour
///
/// There are some cases where you might want to define additional behaviour for a message.
/// For example, if the message contains [`Entities`](Entity), you need to specify how those en
/// entities will be mapped from the remote world to the local world.
///
/// Provided that your type implements [`MapEntities`], you can extend the protocol to support this behaviour, by
/// calling the [`add_map_entities`](MessageRegistration::add_map_entities) method.
///
/// ```rust
/// use bevy::ecs::entity::{EntityMapper, MapEntities};
/// use bevy::prelude::*;
/// use serde::{Deserialize, Serialize};
/// use lightyear::prelude::*;
/// # use lightyear_transport::channel::builder::ChannelDirection;
///
/// #[derive(Serialize, Deserialize, Clone)]
/// struct MyMessage(Entity);
///
/// impl MapEntities for MyMessage {
///    fn map_entities<M: EntityMapper>(&mut self, entity_map: &mut M) {
///        self.0 = entity_map.get_mapped(self.0);
///    }
/// }
///
/// fn add_messages(app: &mut App) {
///   app.register_message::<MyMessage>(ChannelDirection::Bidirectional)
///       .add_map_entities();
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct MessageRegistry {
    pub(crate) send_metadata: HashMap<MessageKind, SendMessageMetadata>,
    pub(crate) receive_metadata: HashMap<MessageKind, ReceiveMessageMetadata>,
    pub(crate) serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub(crate) kind_map: TypeMapper<MessageKind>,
}

impl MessageRegistry {
    pub fn is_registered<M: 'static>(&self) -> bool {
        self.kind_map.net_id(&MessageKind::of::<M>()).is_some()
    }

    /// Register a message for serialization/deserialization
    pub(crate) fn register_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
    ) {
        self.register_message_custom_serde::<M>(SerializeFns::<M>::default())
    }

    pub(crate) fn register_message_custom_serde<M: Message>(&mut self, serialize_fns: SerializeFns<M>) {
        let message_kind = self.kind_map.add::<M>();
        self.serialize_fns_map.insert(
            message_kind,
            ErasedSerializeFns::new_custom_serde::<M>(serialize_fns),
        );
    }

    pub(crate) fn register_sender<M: Message>(&mut self, component_id: ComponentId) {
        self.send_metadata.insert(MessageKind::of::<M>(), SendMessageMetadata {
            component_id,
            send_message_fn: MessageSender::<M>::send_message_typed
        });
    }

    pub(crate) fn register_receiver<M: Message>(&mut self, component_id: ComponentId) {
        self.receive_metadata.insert(MessageKind::of::<M>(), ReceiveMessageMetadata {
            component_id,
            receive_message_fn: MessageReceiver::<M>::receive_message_typed,
        });
    }

    pub(crate) fn try_add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        if let Some(erased_fns) = self.serialize_fns_map.get_mut(&kind) {
            erased_fns.add_map_entities::<M>();
        }
    }

    pub(crate) fn add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get_mut(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.add_map_entities::<M>();
    }

    /// Returns true if we have a registered `map_entities` function for this message type
    pub(crate) fn is_map_entities<M: 'static>(&self) -> bool {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.map_entities.is_some()
    }

    pub(crate) fn serialize<M: Message>(
        &self,
        message: &M,
        writer: &mut Writer,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        let net_id = self.kind_map.net_id(&kind).unwrap();
        net_id.to_bytes(writer)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe {
            erased_fns.serialize(message, writer, entity_map)?;
        }
        Ok(())
    }

    pub(crate) fn deserialize<M: Message>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<M, MessageError> {
        let net_id = NetId::from_bytes(reader)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(MessageError::NotRegistered)?;
        let erased_fns = self
            .serialize_fns_map
            .get(kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe { erased_fns.deserialize(reader, entity_map) }.map_err(Into::into)
    }

    /// Deserialize the message bytes
    /// (We have already deserialized the NetId)
    pub(crate) fn raw_deserialize<M: Message>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<M, MessageError> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe { erased_fns.deserialize(reader, entity_map) }.map_err(Into::into)
    }
}

/// Add a message to the list of messages that can be sent
pub trait AppMessageExt {
    fn add_message<M: Message + Serialize + DeserializeOwned>(&mut self);

    fn add_message_custom_serde<M: Message>(&mut self, serialize_fns: SerializeFns<M>);
}

impl AppMessageExt for App {
    fn add_message<M: Message + Serialize + DeserializeOwned>(&mut self) {
        self.add_message_custom_serde::<M>(SerializeFns::<M>::default());
    }

    // TODO: create a MessageRegistration to add MapEntities
    //  also maybe a similar trick for custom serde?
    fn add_message_custom_serde<M: Message>(&mut self, serialize_fns: SerializeFns<M>) {
        if self.world_mut().get_resource_mut::<MessageRegistry>().is_none() {
            self.world_mut().init_resource::<MessageRegistry>();
        }
        let sender_id = self.world_mut().register_component::<MessageSender<M>>();
        let receiver_id = self.world_mut().register_component::<MessageReceiver<M>>();
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        registry.register_message_custom_serde::<M>(serialize_fns);
        registry.register_sender::<M>(sender_id);
        registry.register_receiver::<M>(receiver_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_serde::reader::ReadInteger;
    use lightyear_serde::writer::WriteInteger;
    use lightyear_serde::SerializationError;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message1(pub f32);

    /// Message where we provide our own serialization/deserialization functions
    #[derive(Debug, PartialEq, Clone, Reflect)]
    pub struct Message2(pub f32);

    pub(crate) fn serialize_message2(
        data: &Message2,
        writer: &mut Writer,
    ) -> std::result::Result<(), SerializationError> {
        writer.write_u32(data.0.to_bits())?;
        Ok(())
    }

    pub(crate) fn deserialize_message2(reader: &mut Reader) -> std::result::Result<Message2, SerializationError> {
        let data = f32::from_bits(reader.read_u32()?);
        Ok(Message2(data))
    }

    #[test]
    fn test_serde() {
        let mut registry = MessageRegistry::default();
        registry.kind_map.add::<Message1>();
        registry.serialize_fns_map.insert(
            MessageKind::of::<Message1>(),
            ErasedSerializeFns::new::<Message1>(),
        );

        let message = Message1(1.0);
        let mut writer = Writer::default();
        registry
            .serialize(&message, &mut writer, &mut SendEntityMap::default())
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(message, read);
    }

    #[test]
    fn test_custom_serde() {
        let mut registry = MessageRegistry::default();
        registry.register_message_custom_serde::<Message2>(SerializeFns {
            serialize: serialize_message2,
            deserialize: deserialize_message2,
        });

        let message = Message2(1.0);
        let mut writer = Writer::default();
        registry
            .serialize(&message, &mut writer, &mut SendEntityMap::default())
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(message, read);
    }
}
