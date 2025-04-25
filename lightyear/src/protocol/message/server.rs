use super::MessageKind;
use crate::prelude::{
    Channel, ClientId, ClientReceiveMessage, Message, NetworkTarget, ServerConnectionManager,
    ServerReceiveMessage, ServerSendMessage,
};
use crate::protocol::message::registry::MessageRegistry;
use crate::protocol::message::trigger::TriggerMessage;
use crate::protocol::message::MessageError;
use crate::protocol::registry::NetId;
use crate::protocol::serialize::ErasedSerializeFns;
use crate::serialize::reader::Reader;
use crate::serialize::ToBytes;
use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::app::App;
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::component::ComponentId;
use bevy::platform::collections::HashMap;
use bevy::prelude::{Commands, Entity, Event, Events, FilteredResourcesMut, TypePath, World};

/// Metadata needed to receive/send messages
///
/// We separate client/server to support host-server mode.
#[derive(Debug, Default, Clone, PartialEq, TypePath)]
pub(crate) struct MessageMetadata {
    /// metadata needed to send a message
    /// We use a Vec instead of a HashMap because we need to iterate through all SendMessage<E> events
    pub(crate) send: Vec<SendMessageMetadata>,
    /// metadata needed to receive a message
    pub(crate) receive: HashMap<MessageKind, ReceiveMessageMetadata>,
    pub(crate) receive_trigger: HashMap<MessageKind, ReceiveTriggerMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiveMessageMetadata {
    /// ComponentId of the Events<ReceiveMessage<M>> resource
    pub(crate) component_id: ComponentId,
    pub(crate) receive_message_fn: ReceiveMessageFn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiveTriggerMetadata {
    /// ComponentId of the Events<ReceiveMessage<TriggerMessage<E>>> resource
    pub(crate) component_id: ComponentId,
    pub(crate) receive_trigger_fn: ReceiveTriggerFn,
}

#[derive(Debug, Clone, PartialEq, TypePath)]
pub(crate) struct SendMessageMetadata {
    kind: MessageKind,
    pub(crate) component_id: ComponentId,
    pub(crate) send_fn: SendMessageFn,
    pub(crate) send_local_fn: SendLocalMessageFn,
}

pub(crate) type ReceiveMessageFn = fn(
    &ReceiveMessageMetadata,
    &ErasedSerializeFns,
    &mut FilteredResourcesMut,
    ClientId,
    &mut Reader,
    &mut ReceiveEntityMap,
) -> Result<(), MessageError>;

type ReceiveTriggerFn = fn(server_receive_events: MutUntyped, commands: &mut Commands);

type SendMessageFn = fn(
    &MessageRegistry,
    send_events: MutUntyped,
    connection_manager: &mut ServerConnectionManager,
) -> Result<(), MessageError>;

type SendLocalMessageFn = fn(
    &MessageRegistry,
    send_events: MutUntyped,
    receive_events: MutUntyped,
    connection_manager: &mut ServerConnectionManager,
) -> Result<(), MessageError>;

impl MessageRegistry {
    pub(crate) fn register_server_send<M: Message>(app: &mut App) {
        app.insert_resource(Events::<ServerSendMessage<M>>::default());
        let message_kind = MessageKind::of::<M>();
        let component_id = app
            .world_mut()
            .resource_id::<Events<ServerSendMessage<M>>>()
            .unwrap();
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .server_messages
            .send
            .push(SendMessageMetadata {
                kind: message_kind,
                component_id,
                send_fn: MessageRegistry::server_send_messages_typed::<M>,
                send_local_fn: MessageRegistry::server_send_messages_local_typed::<M>,
            });
    }

    pub(crate) fn register_server_receive<M: Message>(app: &mut App) {
        app.add_event::<ServerReceiveMessage<M>>();
        let message_kind = MessageKind::of::<M>();
        let component_id = app
            .world_mut()
            .resource_id::<Events<ServerReceiveMessage<M>>>()
            .unwrap();
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .server_messages
            .receive
            .insert(
                message_kind,
                ReceiveMessageMetadata {
                    component_id,
                    receive_message_fn: Self::server_receive_message_typed::<M>,
                },
            );
    }

    pub(crate) fn register_server_trigger_receive<E: Message>(app: &mut App) {
        let message_kind = MessageKind::of::<TriggerMessage<E>>();
        let component_id = app
            .world_mut()
            .resource_id::<Events<ServerReceiveMessage<TriggerMessage<E>>>>()
            .unwrap();
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .server_messages
            .receive_trigger
            .insert(
                message_kind,
                ReceiveTriggerMetadata {
                    component_id,
                    receive_trigger_fn: MessageRegistry::server_receive_trigger_typed::<E>,
                },
            );
    }

    /// Gather all messages present in the various `SendMessage<M>` `send_events`
    /// and buffer them in the `MessageManager`
    pub(crate) fn server_send_messages_local(
        &self,
        send_events: &mut FilteredResourcesMut,
        receive_events: &mut FilteredResourcesMut,
        connection_manager: &mut ServerConnectionManager,
    ) -> Result<(), MessageError> {
        self.server_messages.send.iter().try_for_each(|metadata| {
            let send_event = send_events
                .get_mut_by_id(metadata.component_id)
                .expect("SendEvent<M> resource should be registered");
            let receive_event = receive_events
                .get_mut_by_id(
                    self.client_messages
                        .receive
                        .get(&metadata.kind)
                        .unwrap()
                        .component_id,
                )
                .expect("ReceiveEvent<M> resource should be registered");
            (metadata.send_local_fn)(self, send_event, receive_event, connection_manager)
        })
    }

    // TODO: maybe we just store the serialized bytes in the client's MessageManager
    fn server_send_messages_local_typed<M: Message>(
        &self,
        send_events: MutUntyped,
        receive_events: MutUntyped,
        connection_manager: &mut ServerConnectionManager,
    ) -> Result<(), MessageError> {
        // SAFETY: the PtrMut corresponds to the correct resource
        let mut reader = unsafe { send_events.with_type::<Events<ServerSendMessage<M>>>() };
        let mut writer = unsafe { receive_events.with_type::<Events<ClientReceiveMessage<M>>>() };
        let res = reader.drain().try_for_each(|event| {
            if self.is_map_entities::<M>() {
                // we have to serialize the message separately for all clients
                // because of entity mapping
                for connection in crate::server::connection::connected_targets_mut(
                    &mut connection_manager.connections,
                    &event.to,
                )
                .filter(|c| !c.is_local_client())
                {
                    self.serialize::<M>(
                        &event.message,
                        &mut connection.writer,
                        &mut connection
                            .replication_receiver
                            .remote_entity_map
                            .local_to_remote,
                    )?;
                    let bytes = connection.writer.split();
                    connection
                        .message_manager
                        .buffer_send(bytes, event.channel)
                        .map_err(MessageError::from)?;
                }
            } else {
                // we can serialize once for all non-local clients
                self.serialize::<M>(
                    &event.message,
                    &mut connection_manager.writer,
                    &mut SendEntityMap::default(),
                )?;
                let bytes = connection_manager.writer.split();
                for connection in crate::server::connection::connected_targets_mut(
                    &mut connection_manager.connections,
                    &event.to,
                )
                .filter(|c| !c.is_local_client())
                {
                    // this clone is O(1), it just increments the reference count
                    connection
                        .message_manager
                        .buffer_send(bytes.clone(), event.channel)
                        .map_err(MessageError::from)?;
                }
            }
            writer.send(ClientReceiveMessage::<M>::new(
                event.message,
                ClientId::Server,
            ));
            Ok(())
        });
        res
    }

    /// Gather all messages present in the various `SendMessage<M>` `send_events`
    /// and buffer them in the `MessageManager`
    pub(crate) fn server_send_messages(
        &self,
        send_events: &mut FilteredResourcesMut,
        connection_manager: &mut ServerConnectionManager,
    ) -> Result<(), MessageError> {
        self.server_messages.send.iter().try_for_each(|metadata| {
            let send_events = send_events
                .get_mut_by_id(metadata.component_id)
                .expect("SendEvent<M> resource should be registered");
            (metadata.send_fn)(self, send_events, connection_manager)
        })
    }

    fn server_send_messages_typed<M: Message>(
        &self,
        send_events: MutUntyped,
        connection_manager: &mut ServerConnectionManager,
    ) -> Result<(), MessageError> {
        // SAFETY: the PtrMut corresponds to the correct resource
        let mut reader = unsafe { send_events.with_type::<Events<ServerSendMessage<M>>>() };
        let res = reader.drain().try_for_each(|event| {
            // we have to serialize the message separately for all clients
            // because of entity mapping
            if self.is_map_entities::<M>() {
                for connection in crate::server::connection::connected_targets_mut(
                    &mut connection_manager.connections,
                    &event.to,
                ) {
                    self.serialize::<M>(
                        &event.message,
                        &mut connection.writer,
                        &mut connection
                            .replication_receiver
                            .remote_entity_map
                            .local_to_remote,
                    )?;
                    let bytes = connection.writer.split();
                    connection
                        .message_manager
                        .buffer_send(bytes, event.channel)
                        .map_err(MessageError::from)?;
                }
            } else {
                // we can serialize once for all clients
                self.serialize::<M>(
                    &event.message,
                    &mut connection_manager.writer,
                    &mut SendEntityMap::default(),
                )?;
                let bytes = connection_manager.writer.split();
                for connection in crate::server::connection::connected_targets_mut(
                    &mut connection_manager.connections,
                    &event.to,
                ) {
                    // this clone is O(1), it just increments the reference count
                    connection
                        .message_manager
                        .buffer_send(bytes.clone(), event.channel)
                        .map_err(MessageError::from)?;
                }
            }
            Ok(())
        });
        res
    }

    pub(crate) fn server_receive_message(
        &self,
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
            .server_messages
            .receive
            .get(kind)
            .ok_or(MessageError::NotRegistered)?;
        let serialize_metadata = self
            .serialize_fns_map
            .get(kind)
            .ok_or(MessageError::NotRegistered)?;
        (receive_metadata.receive_message_fn)(
            receive_metadata,
            serialize_metadata,
            resources,
            from,
            reader,
            entity_map,
        )
    }

    /// Internal function of type ReceiveMessageFn (used for type-erasure)
    pub(crate) fn server_receive_message_typed<M: Message>(
        receive_metadata: &ReceiveMessageMetadata,
        serialize_metadata: &ErasedSerializeFns,
        events: &mut FilteredResourcesMut,
        from: ClientId,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        // we deserialize the message and send a MessageEvent
        let message = unsafe { serialize_metadata.deserialize::<M>(reader, entity_map)? };
        let events = events
            .get_mut_by_id(receive_metadata.component_id)
            .map_err(|_| MessageError::NotRegistered)?;
        // SAFETY: the component_id corresponds to the Events<MessageEvent<M>> resource
        let mut events = unsafe { events.with_type::<Events<ServerReceiveMessage<M>>>() };
        events.send(ServerReceiveMessage::new(message, from));
        Ok(())
    }

    pub(crate) fn server_receive_trigger(
        &self,
        receive_events: MutUntyped,
        receive_metadata: &ReceiveTriggerMetadata,
        commands: &mut Commands,
    ) {
        (receive_metadata.receive_trigger_fn)(receive_events, commands)
    }

    pub(crate) fn server_receive_trigger_typed<E: Message>(
        server_receive_events: MutUntyped,
        commands: &mut Commands,
    ) {
        let mut events = unsafe {
            server_receive_events.with_type::<Events<ServerReceiveMessage<TriggerMessage<E>>>>()
        };
        events.drain().for_each(|event| {
            commands.trigger_targets(
                ServerReceiveMessage::new(event.message.event, event.from),
                event.message.target_entities,
            );
        });
    }
}

/// Trait to send remote triggers to the clients
pub trait ServerTriggerExt {
    fn server_trigger<C: Channel>(&mut self, event: impl Event, to: NetworkTarget);

    fn server_trigger_with_targets<C: Channel>(
        &mut self,
        event: impl Event,
        to: NetworkTarget,
        targets: Vec<Entity>,
    );
}

impl ServerTriggerExt for Commands<'_, '_> {
    fn server_trigger<C: Channel>(&mut self, event: impl Event, to: NetworkTarget) {
        self.server_trigger_with_targets::<C>(event, to, vec![]);
    }

    fn server_trigger_with_targets<C: Channel>(
        &mut self,
        event: impl Event,
        to: NetworkTarget,
        targets: Vec<Entity>,
    ) {
        self.send_event(ServerSendMessage::new_with_target::<C>(
            TriggerMessage {
                event,
                target_entities: targets,
            },
            to,
        ));
    }
}

impl ServerTriggerExt for World {
    fn server_trigger<C: Channel>(&mut self, event: impl Event, to: NetworkTarget) {
        self.server_trigger_with_targets::<C>(event, to, vec![]);
    }

    fn server_trigger_with_targets<C: Channel>(
        &mut self,
        event: impl Event,
        to: NetworkTarget,
        targets: Vec<Entity>,
    ) {
        self.send_event(ServerSendMessage::new_with_target::<C>(
            TriggerMessage {
                event,
                target_entities: targets,
            },
            to,
        ));
    }
}
