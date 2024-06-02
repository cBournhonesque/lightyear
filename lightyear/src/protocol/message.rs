use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::Debug;

use crate::client::config::ClientConfig;
use crate::client::message::add_server_to_client_message;
use crate::prelude::{
    client, server, Channel,
};
use bevy::prelude::{
    App, Resource, TypePath,
};
use bevy::reflect::Map;
use bevy::utils::HashMap;
use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};
use serde::Serialize;
use tracing::{debug, error};

use crate::packet::message::Message;
use crate::prelude::server::ServerConfig;
use crate::prelude::{ChannelDirection};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::serialize::ErasedSerializeFns;
use crate::protocol::{BitSerializable};
use crate::serialize::bitcode::reader::BitcodeReader;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::serialize::RawData;
use crate::server::message::add_client_to_server_message;
use crate::shared::replication::entity_map::EntityMap;
use crate::shared::replication::resources::DespawnResource;

#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    #[error("message is not registered in the protocol")]
    NotRegistered,
    #[error("missing serialization functions for message")]
    MissingSerializationFns,
    #[error(transparent)]
    Bitcode(#[from] bitcode::Error),
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

/// A [`Resource`] that will keep track of all the [`Message`]s that can be sent over the network.
/// A [`Message`] is any type that is serializable and deserializable.
///
///
/// ### Adding Messages
///
/// You register messages by calling the [`add_message`](AppMessageExt::add_message) method directly on the App.
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
///   app.add_message::<MyMessage>(ChannelDirection::Bidirectional);
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
/// #[derive(Serialize, Deserialize)]
/// struct MyMessage(Entity);
///
/// impl MapEntities for MyMessage {
///    fn map_entities<M: EntityMapper>(&mut self, entity_map: &mut M) {
///        self.0 = entity_map.map_entity(self.0);
///    }
/// }
///
/// fn add_messages(app: &mut App) {
///   app.add_message::<MyMessage>(ChannelDirection::Bidirectional)
///       .add_map_entities();
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct MessageRegistry {
    typed_map: HashMap<MessageKind, MessageType>,
    serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub(crate) kind_map: TypeMapper<MessageKind>,
}

fn register_message_send<M: Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world.get_resource::<ClientConfig>().is_some();
    let is_server = app.world.get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_server {
                add_client_to_server_message::<M>(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_client {
                add_server_to_client_message::<M>(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_message_send::<M>(app, ChannelDirection::ClientToServer);
            register_message_send::<M>(app, ChannelDirection::ServerToClient);
        }
    }
}

fn register_resource_send<R: Resource + Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world.get_resource::<ClientConfig>().is_some();
    let is_server = app.world.get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_client {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    client::ConnectionManager,
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
    app: &'a mut App,
    _marker: std::marker::PhantomData<M>,
}

impl<M> MessageRegistration<'_, M> {
    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities(self) -> Self
    where
        M: MapEntities + 'static,
    {
        let mut registry = self.app.world.resource_mut::<MessageRegistry>();
        registry.add_map_entities::<M>();
        self
    }
}

pub(crate) trait AppMessageInternalExt {
    /// Function used internally to register a Message with a specific [`MessageType`]
    fn add_message_internal<M: Message>(
        &mut self,
        direction: ChannelDirection,
        message_type: MessageType,
    ) -> MessageRegistration<'_, M>;
}

impl AppMessageInternalExt for App {
    fn add_message_internal<M: Message>(
        &mut self,
        direction: ChannelDirection,
        message_type: MessageType,
    ) -> MessageRegistration<'_, M> {
        let mut registry = self.world.resource_mut::<MessageRegistry>();
        if !registry.is_registered::<M>() {
            registry.add_message::<M>(message_type);
        }
        debug!("register message {}", std::any::type_name::<M>());
        register_message_send::<M>(self, direction);
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
    fn add_message<M: Message>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M>;

    /// Registers the resource in the Registry
    /// This resource can now be sent over the network.
    fn register_resource<R: Resource + Message>(&mut self, direction: ChannelDirection);
}

impl AppMessageExt for App {
    fn add_message<M: Message>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M> {
        self.add_message_internal(direction, MessageType::Normal)
    }

    /// Register a resource to be automatically replicated over the network
    fn register_resource<R: Resource + Message>(&mut self, direction: ChannelDirection) {
        self.add_message::<R>(direction);
        self.add_message::<DespawnResource<R>>(direction);
        register_resource_send::<R>(self, direction)
    }
}

impl MessageRegistry {
    pub(crate) fn message_type(&self, net_id: NetId) -> MessageType {
        let kind = self.kind_map.kind(net_id).unwrap();
        self.typed_map
            .get(kind)
            .map_or(MessageType::Normal, |message_type| *message_type)
    }

    pub fn is_registered<M: 'static>(&self) -> bool {
        self.kind_map.net_id(&MessageKind::of::<M>()).is_some()
    }

    pub(crate) fn add_message<M: Message>(&mut self, message_type: MessageType) {
        let message_kind = self.kind_map.add::<M>();
        self.serialize_fns_map
            .insert(message_kind, ErasedSerializeFns::new::<M>());
        self.typed_map.insert(message_kind, message_type);
    }

    pub(crate) fn try_add_map_entities<M: MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        if let Some(erased_fns) = self.serialize_fns_map.get_mut(&kind) {
            erased_fns.add_map_entities::<M>();
        }
    }

    pub(crate) fn add_map_entities<M: MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get_mut(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.add_map_entities::<M>();
    }

    pub(crate) fn serialize<M: Message>(
        &self,
        message: &M,
        writer: &mut BitcodeWriter,
    ) -> Result<RawData, MessageError> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        let net_id = self.kind_map.net_id(&kind).unwrap();
        writer.start_write();
        writer.encode(net_id, Fixed)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe {
            erased_fns.serialize(message, writer)?;
        }
        Ok(writer.finish_write().to_vec())
    }

    pub(crate) fn deserialize<M: Message>(
        &self,
        reader: &mut BitcodeReader,
        entity_map: &mut EntityMap,
    ) -> Result<M, MessageError> {
        let net_id = reader.decode::<NetId>(Fixed)?;
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
