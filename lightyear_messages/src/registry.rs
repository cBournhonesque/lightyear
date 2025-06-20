use crate::receive::{ClearMessageFn, MessageReceiver, ReceiveMessageFn};
use crate::send::{MessageSender, SendLocalMessageFn, SendMessageFn};
use crate::{Message, MessageNetId};
use bevy::ecs::component::ComponentId;
use bevy::ecs::entity::MapEntities;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::any::TypeId;
use core::cell::UnsafeCell;
use core::hash::Hash;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::network::NetId;
use lightyear_serde::entity_map::{ReceiveEntityMap, RemoteEntityMap, SendEntityMap};
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::{
    ContextDeserializeFn, ContextDeserializeFns, ContextSerializeFn, ContextSerializeFns,
    DeserializeFn, ErasedSerializeFns, SerializeFn, SerializeFns,
};
use lightyear_serde::writer::Writer;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_transport::channel::ChannelKind;
use lightyear_utils::registry::{RegistryHash, RegistryHasher, TypeKind, TypeMapper};
use serde::Serialize;
use serde::de::DeserializeOwned;

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
    UnrecognizedMessageId(MessageNetId),
    #[error(transparent)]
    TransportError(#[from] lightyear_transport::error::TransportError),
}

/// [`MessageKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
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

use crate::receive_trigger::ReceiveTriggerFn;
use crate::send_trigger::{SendLocalTriggerFn, SendTriggerFn};

#[derive(Debug, Clone)]
pub struct ReceiveMessageMetadata {
    /// ComponentId of the [`MessageReceiver<M>`] component (used if not a trigger)
    pub(crate) component_id: ComponentId,
    pub(crate) receive_message_fn: ReceiveMessageFn,
    pub(crate) message_clear_fn: ClearMessageFn,
}

#[derive(Debug, Clone, TypePath)]
pub(crate) struct SendMessageMetadata {
    /// ComponentId of the [`MessageSender<M>`] component
    pub(crate) component_id: ComponentId,
    pub(crate) send_message_fn: SendMessageFn,
    pub(crate) send_local_message_fn: SendLocalMessageFn,
}

#[derive(Debug, Clone, TypePath)]
pub(crate) struct SendTriggerMetadata {
    /// ComponentId of the [`TriggerSender<M>`](crate::send_trigger::TriggerSender) component
    pub(crate) component_id: ComponentId,
    pub(crate) send_trigger_fn: SendTriggerFn,
    pub(crate) send_local_trigger_fn: SendLocalTriggerFn,
}

/// A [`Resource`] that will keep track of all the [`Message`]s that can be sent over the network.
/// A [`Message`] is any type that is serializable and deserializable.
///
///
/// ### Adding Messages
///
/// You register messages by calling the [`add_message`](AppMessageExt::add_message) method directly on the App.
/// You can provide a [`NetworkDirection`] to specify if the message should be sent from the client to the server, from the server to the client, or both.
///
/// ```rust,ignore
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
/// ```rust,ignore
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
#[derive(Debug, Default, Clone, Resource, TypePath)]
pub struct MessageRegistry {
    pub(crate) send_metadata: HashMap<MessageKind, SendMessageMetadata>,
    pub(crate) send_trigger_metadata: HashMap<MessageKind, SendTriggerMetadata>,
    pub(crate) receive_metadata: HashMap<MessageKind, ReceiveMessageMetadata>,
    pub(crate) receive_trigger: HashMap<MessageKind, ReceiveTriggerFn>,
    pub serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub kind_map: TypeMapper<MessageKind>,
    hasher: RegistryHasher,
}

pub struct Context {
    registry: MessageRegistry,
    entity_mapper: UnsafeCell<RemoteEntityMap>,
}

fn mapped_context_serialize<M: MapEntities + Clone>(
    mapper: &mut SendEntityMap,
    message: &M,
    writer: &mut Writer,
    serialize_fn: SerializeFn<M>,
) -> core::result::Result<(), SerializationError> {
    let mut message = message.clone();
    message.map_entities(mapper);
    serialize_fn(&message, writer)
}

fn mapped_context_deserialize<M: MapEntities>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize_fn: DeserializeFn<M>,
) -> core::result::Result<M, SerializationError> {
    let mut message = deserialize_fn(reader)?;
    message.map_entities(mapper);
    Ok(message)
}

impl MessageRegistry {
    pub(crate) fn register_message<M: Message, I: 'static>(
        &mut self,
        serialize: ContextSerializeFns<SendEntityMap, M, I>,
        deserialize: ContextDeserializeFns<ReceiveEntityMap, M, I>,
    ) {
        self.hasher.hash::<M>();
        let message_kind = self.kind_map.add::<I>();
        self.serialize_fns_map.insert(
            message_kind,
            ErasedSerializeFns::new::<SendEntityMap, ReceiveEntityMap, M, I>(
                serialize,
                deserialize,
            ),
        );
    }

    pub(crate) fn register_sender<M: Message>(&mut self, component_id: ComponentId) {
        self.send_metadata.insert(
            MessageKind::of::<M>(),
            SendMessageMetadata {
                component_id,
                send_message_fn: MessageSender::<M>::send_message_typed,
                send_local_message_fn: MessageSender::<M>::send_local_message_typed,
            },
        );
    }

    pub(crate) fn register_receiver<M: Message>(&mut self, component_id: ComponentId) {
        self.receive_metadata.insert(
            MessageKind::of::<M>(),
            ReceiveMessageMetadata {
                component_id,
                receive_message_fn: MessageReceiver::<M>::receive_message_typed,
                message_clear_fn: MessageReceiver::<M>::clear_typed,
            },
        );
    }

    pub(crate) fn is_map_entities<M: 'static>(&self) -> Result<bool> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        Ok(erased_fns.map_entities.is_some())
    }

    pub(crate) fn add_map_entities<
        M: Clone + MapEntities + 'static,
        I: Clone + MapEntities + 'static,
    >(
        &mut self,
        context_serialize: ContextSerializeFn<SendEntityMap, M, I>,
        context_deserialize: ContextDeserializeFn<ReceiveEntityMap, M, I>,
    ) {
        let kind = MessageKind::of::<I>();
        let erased_fns = self
            .serialize_fns_map
            .get_mut(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.add_map_entities::<I>();
        erased_fns.context_serialize = unsafe { core::mem::transmute(context_serialize) };
        erased_fns.context_deserialize = unsafe { core::mem::transmute(context_deserialize) };
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
        unsafe {
            erased_fns.serialize::<SendEntityMap, M, M>(message, writer, entity_map)?;
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
        unsafe {
            erased_fns
                .deserialize::<ReceiveEntityMap, M, M>(reader, entity_map)
                .map_err(Into::into)
        }
    }

    pub fn finish(&mut self) -> RegistryHash {
        self.hasher.finish()
    }
}

pub struct MessageRegistration<'a, M> {
    pub app: &'a mut App,
    pub(crate) _marker: core::marker::PhantomData<M>,
}

impl<'a, M: Message> MessageRegistration<'a, M> {
    #[cfg(feature = "test_utils")]
    pub fn new(app: &'a mut App) -> Self {
        Self {
            app,
            _marker: core::marker::PhantomData,
        }
    }

    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities(&mut self) -> &mut Self
    where
        M: Clone + MapEntities + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<MessageRegistry>();
        registry.add_map_entities::<M, M>(mapped_context_serialize, mapped_context_deserialize);
        self
    }

    pub fn add_direction(&mut self, direction: NetworkDirection) -> &mut Self {
        #[cfg(feature = "client")]
        self.add_client_direction(direction);
        #[cfg(feature = "server")]
        self.add_server_direction(direction);
        self
    }
}

/// Add messages or triggers to the list of types that can be sent.
pub trait AppMessageExt {
    /// Register a regular message type `M`.
    /// This adds `MessageSender<M>` and `MessageReceiver<M>` components.
    fn add_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
    ) -> MessageRegistration<'_, M>;

    /// Register a regular message type `M` with custom serialization functions.
    fn add_message_custom_serde<M: Message>(
        &mut self,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M>;

    #[doc(hidden)]
    /// Register a regular message type `M` that uses `ToBytes` for serialization.
    fn add_message_to_bytes<M: Message + ToBytes>(&mut self) -> MessageRegistration<'_, M>;
}

impl AppMessageExt for App {
    fn add_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
    ) -> MessageRegistration<'_, M> {
        self.add_message_custom_serde::<M>(SerializeFns::<M>::default())
    }

    fn add_message_custom_serde<M: Message>(
        &mut self,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M> {
        if self
            .world_mut()
            .get_resource_mut::<MessageRegistry>()
            .is_none()
        {
            self.world_mut().init_resource::<MessageRegistry>();
        }
        // Register components for sending/receiving M
        let sender_id = self.world_mut().register_component::<MessageSender<M>>();
        let receiver_id = self.world_mut().register_component::<MessageReceiver<M>>();

        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        // Register M for serialization/deserialization
        registry.register_message::<M, M>(
            ContextSerializeFns::new(serialize_fns.serialize),
            ContextDeserializeFns::new(serialize_fns.deserialize),
        );
        // Register sender/receiver metadata for M, ensuring trigger_fn is None
        registry.register_sender::<M>(sender_id);
        registry.register_receiver::<M>(receiver_id); // This sets trigger_fn to None by default

        MessageRegistration {
            app: self,
            _marker: Default::default(),
        }
    }

    fn add_message_to_bytes<M: Message + ToBytes>(&mut self) -> MessageRegistration<'_, M> {
        self.add_message_custom_serde::<M>(SerializeFns::<M>::with_to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_serde::SerializationError;
    use lightyear_serde::reader::ReadInteger;
    use lightyear_serde::writer::WriteInteger;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message1(pub f32);

    /// Message where we provide our own serialization/deserialization functions
    #[derive(Debug, PartialEq, Clone, Reflect)]
    pub struct Message2(pub f32);

    pub(crate) fn serialize_message2(
        data: &Message2,
        writer: &mut Writer,
    ) -> core::result::Result<(), SerializationError> {
        writer.write_u32(data.0.to_bits())?;
        Ok(())
    }

    pub(crate) fn deserialize_message2(
        reader: &mut Reader,
    ) -> core::result::Result<Message2, SerializationError> {
        let data = f32::from_bits(reader.read_u32()?);
        Ok(Message2(data))
    }

    /// Message where we provide our own serialization/deserialization functions
    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message3(pub Entity);

    impl MapEntities for Message3 {
        fn map_entities<M: EntityMapper>(&mut self, entity_map: &mut M) {
            self.0 = entity_map.get_mapped(self.0);
        }
    }

    #[test]
    fn test_serde() {
        let mut registry = MessageRegistry::default();
        registry.kind_map.add::<Message1>();
        registry.serialize_fns_map.insert(
            MessageKind::of::<Message1>(),
            ErasedSerializeFns::new(
                ContextSerializeFns::<(), _>::new(SerializeFns::<Message1>::default().serialize),
                ContextDeserializeFns::<(), _>::new(
                    SerializeFns::<Message1>::default().deserialize,
                ),
            ),
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
        registry.register_message::<Message2, _>(
            ContextSerializeFns::new(serialize_message2),
            ContextDeserializeFns::new(deserialize_message2),
        );

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

    #[test]
    fn test_entity_map() {
        let mut registry = MessageRegistry::default();
        registry.kind_map.add::<Message3>();
        registry.serialize_fns_map.insert(
            MessageKind::of::<Message3>(),
            ErasedSerializeFns::new(
                ContextSerializeFns::<SendEntityMap, _>::new(
                    SerializeFns::<Message3>::default().serialize,
                ),
                ContextDeserializeFns::<ReceiveEntityMap, _>::new(
                    SerializeFns::<Message3>::default().deserialize,
                ),
            ),
        );
        registry.add_map_entities(
            mapped_context_serialize::<Message3>,
            mapped_context_deserialize::<Message3>,
        );

        let message = Message3(Entity::from_raw(1));
        let mut writer = Writer::default();
        let mut entity_map = SendEntityMap::default();
        entity_map.set_mapped(Entity::from_raw(1), Entity::from_raw(2));
        registry
            .serialize(&message, &mut writer, &mut entity_map)
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize::<Message3>(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(read.0, Entity::from_raw(2));
    }
}
