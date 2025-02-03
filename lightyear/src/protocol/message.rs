use crate::client::config::ClientConfig;
use crate::inputs::native::InputMessage;
use crate::packet::message::Message;
use crate::packet::message_manager::MessageManager;
use crate::prelude::server::ServerConfig;
use crate::prelude::{client, server, ClientId, ServerReceiveMessage};
use crate::prelude::{ChannelDirection, UserAction};
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::serialize::{ErasedSerializeFns, SerializeFns};
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::ToBytes;
use crate::server::input::native::InputBuffers;
use crate::shared::events::message::{ReceiveMessage, SendMessage};
use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
use crate::shared::replication::resources::DespawnResource;
use crate::shared::sets::{ClientMarker, ServerMarker};
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::component::ComponentId;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{App, Commands, Events, FilteredResourcesMut, Resource, TypePath, World};
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
    #[error(transparent)]
    Packet(#[from] crate::packet::error::PacketError),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum MessageType {
    /// This is a message for a [`LeafwingUserAction`](crate::inputs::leafwing::LeafwingUserAction)
    #[cfg(feature = "leafwing")]
    LeafwingInput,
    /// This is a message for a [`UserAction`]
    NativeInput,
    /// This is not an input message, but a regular [`Message`]
    #[default]
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
    /// metadata needed to send a message
    /// We use a Vec instead of a HashMap because we need to iterate through all SendMessage<E> events
    pub(crate) client_send_metadata: Vec<SendMessageMetadata>,
    /// metadata needed to receive a message
    pub(crate) client_receive_metadata: HashMap<MessageKind, ReceiveMessageMetadata>,
    /// metadata needed to send a message
    /// We use a Vec instead of a HashMap because we need to iterate through all SendMessage<E> events
    pub(crate) server_send_metadata: Vec<SendMessageMetadata>,
    /// metadata needed to receive a message
    pub(crate) server_receive_metadata: HashMap<MessageKind, ReceiveMessageMetadata>,
    pub(crate) metadata: HashMap<MessageKind, Metadata>,
    pub(crate) serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub(crate) kind_map: TypeMapper<MessageKind>,
}

#[derive(Debug, Default, Clone, PartialEq, TypePath)]
pub(crate) struct Metadata {
    pub(crate) message_type: MessageType
}


pub(crate) type ReceiveMessageFn = fn(
    &ReceiveMessageMetadata,
    &ErasedSerializeFns,
    &mut Commands,
    &mut FilteredResourcesMut,
    ClientId,
    &mut Reader,
    &mut ReceiveEntityMap,
) -> Result<(), MessageError>;

type SendMessageFn = fn(
    &MessageRegistry,
    send_events: MutUntyped,
    message_manager: &mut MessageManager,
    entity_map: &mut SendEntityMap,
    client_to_server: bool,
) -> Result<(), MessageError>;

type SendHostServerMessageFn = fn(
    &MessageRegistry,
    send_events: MutUntyped,
    receive_events: MutUntyped,
    sender: ClientId,
);

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiveMessageMetadata {
    pub(crate) message_type: MessageType,
    /// ComponentId of the Events<MessageEvent<M>> resource
    pub(crate) component_id: ComponentId,
    pub(crate) receive_message_fn: ReceiveMessageFn,
}

#[derive(Debug, Clone, PartialEq, TypePath)]
pub struct SendMessageMetadata {
    kind: MessageKind,
    pub(crate) message_type: MessageType,
    pub(crate) component_id: ComponentId,
    pub(crate) send_fn: SendMessageFn,
    pub(crate) send_host_server_fn: SendHostServerMessageFn,
}



/// Register the message-receive metadata for a given message M
fn register_message<M: Message>(
    app: &mut App,
    direction: ChannelDirection,
    message_type: MessageType,
) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_server {
                app.insert_resource(Events::<ReceiveMessage<M, ServerMarker>>::default());
                let message_kind = MessageKind::of::<M>();
                let component_id = app
                    .world_mut()
                    .resource_id::<Events<ReceiveMessage<M, ServerMarker>>>().unwrap();
                let receive_message_fn: ReceiveMessageFn = MessageRegistry::receive_message_typed::<M, ServerMarker>;
                app.world_mut()
                    .resource_mut::<MessageRegistry>()
                    .server_receive_metadata
                    .insert(
                        message_kind,
                        ReceiveMessageMetadata {
                            message_type,
                            component_id,
                            receive_message_fn,
                        },
                    );
            };
            if is_client {
                app.insert_resource(Events::<SendMessage<M, ClientMarker>>::default());
                let message_kind = MessageKind::of::<M>();
                let component_id = app
                    .world_mut()
                    .resource_id::<Events<SendMessage<M, ClientMarker>>>().unwrap();
                if message_type == MessageType::Normal || message_type == MessageType::Event {
                    app.world_mut()
                        .resource_mut::<MessageRegistry>()
                        .client_send_metadata
                        .push(SendMessageMetadata {
                            kind: message_kind,
                            message_type,
                            component_id,
                            send_fn: MessageRegistry::send_message_typed::<M, ClientMarker>,
                            send_host_server_fn: MessageRegistry::send_host_server_typed::<M, ClientMarker, ServerMarker>,
                        });
                }
            };
        }
        ChannelDirection::ServerToClient => {
            if is_client {
                app.insert_resource(Events::<ReceiveMessage<M, ClientMarker>>::default());
                let message_kind = MessageKind::of::<M>();
                let component_id = app
                    .world_mut()
                    .resource_id::<Events<ReceiveMessage<M, ClientMarker>>>().unwrap();
                let receive_message_fn: ReceiveMessageFn = MessageRegistry::receive_message_typed::<M, ClientMarker>;
                app.world_mut()
                    .resource_mut::<MessageRegistry>()
                    .client_receive_metadata
                    .insert(
                        message_kind,
                        ReceiveMessageMetadata {
                            message_type,
                            component_id,
                            receive_message_fn,
                        },
                    );
            };
            if is_server {
                app.insert_resource(Events::<SendMessage<M, ServerMarker>>::default());
                let message_kind = MessageKind::of::<M>();
                let component_id = app
                    .world_mut()
                    .resource_id::<Events<SendMessage<M, ServerMarker>>>().unwrap();
                if message_type == MessageType::Normal || message_type == MessageType::Event {
                    app.world_mut()
                        .resource_mut::<MessageRegistry>()
                        .server_send_metadata
                        .push(SendMessageMetadata {
                            kind: message_kind,
                            message_type,
                            component_id,
                            send_fn: MessageRegistry::send_message_typed::<M, ServerMarker>,
                            send_host_server_fn: MessageRegistry::send_host_server_typed::<M, ServerMarker, ServerMarker>,
                        });
                }
            }
        }
        ChannelDirection::Bidirectional => {
            register_message::<M>(app, ChannelDirection::ClientToServer, message_type);
            register_message::<M>(app, ChannelDirection::ServerToClient, message_type);
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
            let message_kind = registry.kind_map.add::<M>();
            registry.serialize_fns_map
                .insert(message_kind, ErasedSerializeFns::new::<M>());
            registry.metadata.insert(message_kind, Metadata {
                message_type
            });
        }
        error!("register message {}. Kind: {:?}", std::any::type_name::<M>(), MessageKind::of::<M>());
        register_message::<M>(self, direction, MessageType::Normal);
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
        register_message::<M>(self, direction, message_type);
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
        self.serialize_fns_map
            .get(kind)
            .map_or(MessageType::Normal, |metadata| metadata.message_type)
    }

    pub fn is_registered<M: 'static>(&self) -> bool {
        self.kind_map.net_id(&MessageKind::of::<M>()).is_some()
    }

    pub(crate) fn add_message<M: Message + Serialize + DeserializeOwned>(&mut self, message_type: MessageType) {
        let message_kind = self.kind_map.add::<M>();
        self.serialize_fns_map
            .insert(message_kind, ErasedSerializeFns::new::<M>());
        self.metadata.insert(message_kind, Metadata {
            message_type
        });
    }

    /// Gather all messages present in the various `SendMessage<M>` `send_events`
    /// and buffer them in the `MessageManager`
    pub(crate) fn send_host_server_messages(
        &self,
        send_events: &mut FilteredResourcesMut,
        receive_events: &mut FilteredResourcesMut,
        sender: ClientId,
    ) {
        self.send_metadata.iter().for_each(|metadata| {
            let send_event = send_events.get_mut_by_id(metadata.component_id).expect("SendEvent<M> resource should be registered");
            let receive_event = receive_events.get_mut_by_id(self.receive_metadata.get(&metadata.kind).unwrap().component_id)
                .expect("ReceiveEvent<M> resource should be registered");
            (metadata.send_host_server_fn)(
                self,
                send_event,
                receive_event,
                sender,
            )
        })
    }

    /// Send a message directly from SendMessage to ReceiveMessage
    /// No need to apply entity-mapping since the client and server worlds are the same
    fn send_host_server_typed<M: Message, Send: Message, Receive: Message>(
        &self,
        send_events: MutUntyped,
        receive_events: MutUntyped,
        sender: ClientId,
    ) {
        // SAFETY: the PtrMut corresponds to the correct resource
        let mut reader = unsafe { send_events.with_type::<Events<SendMessage<M, Send>>>() };
        let mut writer = unsafe { receive_events.with_type::<Events<ReceiveMessage<M, Receive>>>() };
        reader.drain().for_each(|event| {
            writer.send(ReceiveMessage::new(event.message, sender));
        });
    }

    /// Gather all messages present in the various `SendMessage<M>` `send_events`
    /// and buffer them in the `MessageManager`
    pub(crate) fn send_messages(
        &self,
        send_events: &mut FilteredResourcesMut,
        manager: &mut MessageManager,
        entity_map: &mut SendEntityMap,
        client_to_server: bool,
    ) -> Result<(), MessageError> {
        self.send_metadata.iter().try_for_each(|metadata| {
            let send_events = send_events.get_mut_by_id(metadata.component_id).expect("SendEvent<M> resource should be registered");
            (metadata.send_fn)(
                self,
                send_events,
                manager,
                entity_map,
                client_to_server
            )
        })
    }

    fn send_message_typed<M: Message, Marker: Message>(
        &self,
        send_events: MutUntyped,
        message_manager: &mut MessageManager,
        entity_map: &mut SendEntityMap,
        client_to_server: bool,
    ) -> Result<(), MessageError> {
        // SAFETY: the PtrMut corresponds to the correct resource
        let mut reader = unsafe { send_events.with_type::<Events<SendMessage<M, Marker>>>() };
        let res = reader.drain().try_for_each(|event| {
            // client->server, include the target in the message for rebroadcasting
            // server->client, no need to include the target
            if client_to_server {
                event.to.to_bytes(&mut message_manager.writer)?;
            }
            self.serialize::<M>(
                &event.message,
                &mut message_manager.writer,
                entity_map
            )?;
            let message_bytes = message_manager.writer.split();
            message_manager.buffer_send(message_bytes, event.channel)?;
            Ok(())
        });
        res
    }

    /// Receive a message from the remote
    /// The message could be:
    /// - a Normal message, in which case we buffer a MessageEvent
    /// - a Event message, in which case we buffer the event directly, or we trigger it
    pub(crate) fn client_receive_message(
        &self,
        // TODO: this param is unneeded, maybe have a separate EventRegistry?
        commands: &mut Commands,
        resources: &mut FilteredResourcesMut,
        from: ClientId,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        let net_id = NetId::from_bytes(reader)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(MessageError::NotRegistered)?;
        let receive_metadata = self
            .client_receive_metadata
            .get(kind)
            .ok_or(MessageError::NotRegistered)?;
        let serialize_metadata = self.serialize_fns_map.get(kind).ok_or(MessageError::NotRegistered)?;
        (receive_metadata.receive_message_fn)(receive_metadata, serialize_metadata,  commands, resources, from, reader, entity_map)
    }

    /// Internal function of type ReceiveMessageFn (used for type-erasure)
    fn receive_message_typed<M: Message, Marker: Message>(
        receive_metadata: &ReceiveMessageMetadata,
        serialize_metadata: &ErasedSerializeFns,
        commands: &mut Commands,
        events: &mut FilteredResourcesMut,
        from: ClientId,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        match receive_metadata.message_type {
            MessageType::Normal => {
                // we deserialize the message and send a MessageEvent
                let message = unsafe { serialize_metadata.deserialize::<M>(reader, entity_map)? };
                let events = events
                    .get_mut_by_id(receive_metadata.component_id)
                    .ok_or(MessageError::NotRegistered)?;
                // SAFETY: the component_id corresponds to the Events<MessageEvent<M>> resource
                let mut events = unsafe { events.with_type::<Events<ReceiveMessage<M, Marker>>>() };
                events.send(ReceiveMessage::new(message, from));
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
        registry.serialize(&message, &mut writer, &mut SendEntityMap::default()).unwrap();
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
            .serialize(&message, &mut writer, &mut map)
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
        });

        let message = Resource2(1.0);
        let mut writer = Writer::default();
        registry.serialize(&message, &mut writer, &mut SendEntityMap::default()).unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(message, read);
    }
}