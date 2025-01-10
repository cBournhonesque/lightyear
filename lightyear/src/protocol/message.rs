use crate::client::config::ClientConfig;
use crate::inputs::native::InputMessage;
use crate::packet::message::Message;
use crate::prelude::server::ServerConfig;
use crate::prelude::{client, server, ClientId, ClientMessageSender, ServerMessageSender};
use crate::prelude::{ChannelDirection, UserAction};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::serialize::{ErasedSerializeFns, SerializeFns};
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::ToBytes;
use crate::server::input::native::InputBuffers;
use crate::shared::events::components::MessageEvent;
use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
use crate::shared::replication::resources::DespawnResource;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{App, Events, Resource, TypePath, World};
use bevy::utils::HashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::any::TypeId;
use std::fmt::Debug;
use tracing::{debug, error, trace};

#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    #[error("the message if of the wrong type")]
    IncorrectType,
    #[error("message is not registered in the protocol")]
    NotRegistered,
    #[error("missing serialization functions for message")]
    MissingSerializationFns,
    #[error(transparent)]
    Serialization(#[from] crate::serialize::SerializationError),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MessageType {
    /// This is a message for a [`LeafwingUserAction`](crate::inputs::leafwing::LeafwingUserAction)
    #[cfg(feature = "leafwing")]
    LeafwingInput,
    /// This is a message for a [`UserAction`]
    NativeInput,
    /// This is not an input message, but a regular [`Message`]
    Normal,
    /// This message is an [`Event`](bevy::prelude::Event), which can get triggered or buffered in an EventWriter on the remote world
    Event,
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
/// For example, if the message contains [`Entities`](bevy::prelude::Entity), you need to specify how those en
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
///
/// #[derive(Serialize, Deserialize, Clone)]
/// struct MyMessage(Entity);
///
/// impl MapEntities for MyMessage {
///    fn map_entities<M: EntityMapper>(&mut self, entity_map: &mut M) {
///        self.0 = entity_map.map_entity(self.0);
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
    /// metadata needed to receive a message
    pub(crate) message_receive_map: HashMap<MessageKind, MessageMetadata>,
    pub(crate) serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub(crate) kind_map: TypeMapper<MessageKind>,
}

pub(crate) type ReceiveMessageFn = fn(
    &MessageRegistry,
    &mut World,
    ClientId,
    &mut Reader,
    &mut ReceiveEntityMap,
) -> Result<(), MessageError>;

#[derive(Debug, Clone, PartialEq)]
pub struct MessageMetadata {
    pub(crate) message_type: MessageType,
    pub(crate) receive_message_fn: ReceiveMessageFn,
}

fn register_message_receive<M: Message>(
    app: &mut App,
    direction: ChannelDirection,
    message_type: MessageType,
) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    let register_fn = |app: &mut App| {
        app.add_event::<MessageEvent<M>>();
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .add_receive_message_metadata::<M>(message_type);
    };
    match direction {
        ChannelDirection::ClientToServer => {
            if is_server {
                register_fn(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_client {
                register_fn(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_message_receive::<M>(app, ChannelDirection::ClientToServer, message_type);
            register_message_receive::<M>(app, ChannelDirection::ServerToClient, message_type);
        }
    }
}

fn register_resource_send<R: Resource + Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_client {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    client::ConnectionManager,
                    ClientMessageSender,
                >(app);
            }
            if is_server {
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    server::ConnectionManager,
                >(app, false);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_server {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    server::ConnectionManager,
                    ServerMessageSender,
                >(app);
            }
            if is_client {
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    client::ConnectionManager,
                >(app, false);
            }
        }
        ChannelDirection::Bidirectional => {
            if is_server {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    server::ConnectionManager,
                    ServerMessageSender,
                >(app);
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    server::ConnectionManager,
                >(app, true);
            }
            if is_client {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    client::ConnectionManager,
                    ClientMessageSender,
                >(app);
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    client::ConnectionManager,
                >(app, true);
            }
            // register_resource_send::<R>(app, ChannelDirection::ClientToServer);
            // register_resource_send::<R>(app, ChannelDirection::ServerToClient);
        }
    }
}

pub struct MessageRegistration<'a, M> {
    pub(crate) app: &'a mut App,
    pub(crate) _marker: std::marker::PhantomData<M>,
}

impl<M> MessageRegistration<'_, M> {
    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities(self) -> Self
    where
        M: Clone + MapEntities + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<MessageRegistry>();
        registry.add_map_entities::<M>();
        self
    }
}

pub(crate) trait AppMessageInternalExt {
    /// Function used internally to register a Message with a specific [`MessageType`]
    fn register_message_internal<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
        message_type: MessageType,
    ) -> MessageRegistration<'_, M>;

    /// Function used internally to register a Message with a specific [`MessageType`]
    /// and a custom [`SerializeFns`] implementation
    fn register_message_internal_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        message_type: MessageType,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M>;
}

impl AppMessageInternalExt for App {
    fn register_message_internal<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
        message_type: MessageType,
    ) -> MessageRegistration<'_, M> {
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        if !registry.is_registered::<M>() {
            registry.add_message::<M>();
        }
        debug!("register message {}", std::any::type_name::<M>());
        register_message_receive::<M>(self, direction, message_type);
        MessageRegistration {
            app: self,
            _marker: std::marker::PhantomData,
        }
    }

    fn register_message_internal_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        message_type: MessageType,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M> {
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        if !registry.is_registered::<M>() {
            registry.add_message_custom_serde::<M>(serialize_fns);
        }
        debug!("register message {}", std::any::type_name::<M>());
        register_message_receive::<M>(self, direction, message_type);
        MessageRegistration {
            app: self,
            _marker: std::marker::PhantomData,
        }
    }
}

/// Add a message to the list of messages that can be sent
pub trait AppMessageExt {
    /// Registers the message in the Registry
    /// This message can now be sent over the network.
    fn register_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M>;

    /// Registers the message in the Registry
    ///
    /// This message can now be sent over the network.
    /// You need to provide your own [`SerializeFns`] for this message
    fn register_message_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M>;

    /// Registers the resource in the Registry
    /// This resource can now be sent over the network.
    fn register_resource<R: Resource + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    );

    /// Registers the resource in the Registry
    ///
    /// This resource can now be sent over the network.
    /// You need to provide your own [`SerializeFns`] for this message
    fn register_resource_custom_serde<R: Resource + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<R>,
    );
}

impl AppMessageExt for App {
    fn register_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M> {
        self.register_message_internal(direction, MessageType::Normal)
    }

    fn register_message_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M> {
        self.register_message_internal_custom_serde(direction, MessageType::Normal, serialize_fns)
    }

    /// Register a resource to be automatically replicated over the network
    fn register_resource<R: Resource + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) {
        self.register_message::<R>(direction);
        self.register_message::<DespawnResource<R>>(direction);
        register_resource_send::<R>(self, direction)
    }

    /// Register a resource to be automatically replicated over the network
    fn register_resource_custom_serde<R: Resource + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<R>,
    ) {
        self.register_message_custom_serde::<R>(direction, serialize_fns);
        self.register_message::<DespawnResource<R>>(direction);
        register_resource_send::<R>(self, direction)
    }
}

impl MessageRegistry {
    pub(crate) fn message_type(&self, net_id: NetId) -> MessageType {
        // TODO this unwrap takes down server if client sends invalid netid.
        //      perhaps return a result from this and handle?
        let kind = self.kind_map.kind(net_id).unwrap();
        self.message_receive_map
            .get(kind)
            .map_or(MessageType::Normal, |metadata| metadata.message_type)
    }

    pub fn is_registered<M: 'static>(&self) -> bool {
        self.kind_map.net_id(&MessageKind::of::<M>()).is_some()
    }

    pub(crate) fn add_message<M: Message + Serialize + DeserializeOwned>(&mut self) {
        let message_kind = self.kind_map.add::<M>();
        self.serialize_fns_map
            .insert(message_kind, ErasedSerializeFns::new::<M>());
    }

    pub(crate) fn add_receive_message_metadata<M: Message>(&mut self, message_type: MessageType) {
        let message_kind = MessageKind::of::<M>();
        let send_message_fn: ReceiveMessageFn = Self::receive_message_internal::<M>;
        self.message_receive_map.insert(
            message_kind,
            MessageMetadata {
                message_type,
                receive_message_fn: send_message_fn,
            },
        );
    }

    /// Receive a message from the remote
    /// The message could be:
    /// - a Normal message, in which case we buffer a MessageEvent
    /// - a Event message, in which case we buffer the event directly, or we trigger it
    pub(crate) fn receive_message(
        &self,
        net_id: NetId,
        world: &mut World,
        from: ClientId,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(MessageError::NotRegistered)?;
        let metadata = self
            .message_receive_map
            .get(kind)
            .ok_or(MessageError::NotRegistered)?;
        (metadata.receive_message_fn)(self, world, from, reader, entity_map)
    }

    /// Internal function of type ReceiveMessageFn (used for type-erasure)
    fn receive_message_internal<M: Message>(
        &self,
        world: &mut World,
        from: ClientId,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        let kind = MessageKind::of::<M>();
        let receive_metadata = self
            .message_receive_map
            .get(&kind)
            .ok_or(MessageError::NotRegistered)?;
        match receive_metadata.message_type {
            MessageType::Normal => {
                // we deserialize the message and send a MessageEvent
                let mut events = world.resource_mut::<Events<MessageEvent<M>>>();
                let message = self.deserialize::<M>(reader, entity_map)?;
                events.send(MessageEvent::new(message, from));
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    pub(crate) fn add_message_custom_serde<M: Message>(&mut self, serialize_fns: SerializeFns<M>) {
        let message_kind = self.kind_map.add::<M>();
        self.serialize_fns_map.insert(
            message_kind,
            ErasedSerializeFns::new_custom_serde::<M>(serialize_fns),
        );
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
        entity_map: Option<&mut SendEntityMap>,
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
}

mod inputs {
    use super::*;

    impl MessageRegistry {
        fn receive_native_input_internal<A: UserAction>(
            &self,
            world: &mut World,
            from: ClientId,
            reader: &mut Reader,
            entity_map: &mut ReceiveEntityMap,
        ) -> Result<(), MessageError> {
            let kind = MessageKind::of::<InputMessage<A>>();
            let receive_metadata = self
                .message_receive_map
                .get(&kind)
                .ok_or(MessageError::NotRegistered)?;
            if receive_metadata.message_type != MessageType::NativeInput {
                error!("Incorrect message type for InputMessage<A>, should be NativeInput");
                return Err(MessageError::IncorrectType);
            }
            if let Some(mut input_buffers) = world.get_resource_mut::<InputBuffers<A>>() {
                let message = self.deserialize::<InputMessage<A>>(reader, entity_map)?;
                trace!("Received input message: {:?}", message);
                input_buffers
                    .buffers
                    .entry(from)
                    .or_default()
                    .1
                    .update_from_message(message);
            }
            Ok(())
        }
    }
}

/// [`MessageKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::protocol::{
        deserialize_resource2, serialize_resource2, ComponentMapEntities, Resource1, Resource2,
    };
    use bevy::prelude::Entity;

    #[test]
    fn test_serde() {
        let mut registry = MessageRegistry::default();
        registry.add_message::<Resource1>();

        let message = Resource1(1.0);
        let mut writer = Writer::default();
        registry.serialize(&message, &mut writer, None).unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(message, read);
    }

    #[test]
    fn test_serde_map() {
        let mut registry = MessageRegistry::default();
        registry.add_message::<ComponentMapEntities>();
        registry.add_map_entities::<ComponentMapEntities>();

        let message = ComponentMapEntities(Entity::from_raw(0));
        let mut writer = Writer::default();
        let mut map = SendEntityMap::default();
        map.insert(Entity::from_raw(0), Entity::from_raw(1));
        registry
            .serialize(&message, &mut writer, Some(&mut map))
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize::<ComponentMapEntities>(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(read, ComponentMapEntities(Entity::from_raw(1)));
    }

    #[test]
    fn test_custom_serde() {
        let mut registry = MessageRegistry::default();
        registry.add_message_custom_serde::<Resource2>(SerializeFns {
            serialize: serialize_resource2,
            deserialize: deserialize_resource2,
            serialize_map_entities: None,
        });

        let message = Resource2(1.0);
        let mut writer = Writer::default();
        registry.serialize(&message, &mut writer, None).unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(message, read);
    }
}
