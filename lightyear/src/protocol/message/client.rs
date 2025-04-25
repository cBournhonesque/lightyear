use super::MessageKind;
use crate::packet::message_manager::MessageManager;
use crate::prelude::{
    Channel, ClientId, ClientReceiveMessage, ClientSendMessage, Message, ServerReceiveMessage,
};
use crate::protocol::message::registry::MessageRegistry;
use crate::protocol::message::trigger::TriggerMessage;
use crate::protocol::message::MessageError;
use crate::protocol::registry::NetId;
use crate::protocol::serialize::ErasedSerializeFns;
use crate::serialize::reader::Reader;
use crate::serialize::ToBytes;
use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
use bevy::app::App;
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::component::ComponentId;
use bevy::platform::collections::HashMap;
use bevy::prelude::{Commands, Entity, Event, Events, FilteredResourcesMut, TypePath, World};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

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
    /// ComponentId of the Events<ReceiveMessage<RemoteTrigger<E>>> resource
    pub(crate) component_id: ComponentId,
    pub(crate) receive_trigger_fn: ReceiveTriggerFn,
}

#[derive(Debug, Clone, PartialEq, TypePath)]
pub(crate) struct SendMessageMetadata {
    kind: MessageKind,
    pub(crate) component_id: ComponentId,
    pub(crate) send_fn: SendMessageFn,
    pub(crate) send_host_server_fn: SendHostServerMessageFn,
}

pub(crate) type ReceiveMessageFn = fn(
    &ReceiveMessageMetadata,
    &ErasedSerializeFns,
    &mut FilteredResourcesMut,
    ClientId,
    &mut Reader,
    &mut ReceiveEntityMap,
) -> Result<(), MessageError>;

type ReceiveTriggerFn = fn(receive_events: MutUntyped, commands: &mut Commands);

type SendMessageFn = fn(
    &MessageRegistry,
    send_events: MutUntyped,
    message_manager: &mut MessageManager,
    entity_map: &mut SendEntityMap,
) -> Result<(), MessageError>;

type SendHostServerMessageFn =
    fn(&MessageRegistry, send_events: MutUntyped, receive_events: MutUntyped, sender: ClientId);

impl MessageRegistry {
    pub(crate) fn register_client_send<M: Message>(app: &mut App) {
        app.insert_resource(Events::<ClientSendMessage<M>>::default());
        let message_kind = MessageKind::of::<M>();
        let component_id = app
            .world_mut()
            .resource_id::<Events<ClientSendMessage<M>>>()
            .unwrap();
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .client_messages
            .send
            .push(SendMessageMetadata {
                kind: message_kind,
                component_id,
                send_fn: MessageRegistry::client_send_message_typed::<M>,
                send_host_server_fn: MessageRegistry::client_send_host_server_typed::<M>,
            });
    }

    pub(crate) fn register_client_receive<M: Message>(app: &mut App) {
        app.add_event::<ClientReceiveMessage<M>>();
        let message_kind = MessageKind::of::<M>();
        let component_id = app
            .world_mut()
            .resource_id::<Events<ClientReceiveMessage<M>>>()
            .unwrap();
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .client_messages
            .receive
            .insert(
                message_kind,
                ReceiveMessageMetadata {
                    component_id,
                    receive_message_fn: Self::client_receive_message_typed::<M>,
                },
            );
    }

    pub(crate) fn register_client_trigger_receive<E: Message>(app: &mut App) {
        let message_kind = MessageKind::of::<TriggerMessage<E>>();
        let component_id = app
            .world_mut()
            .resource_id::<Events<ClientReceiveMessage<TriggerMessage<E>>>>()
            .unwrap();
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .client_messages
            .receive_trigger
            .insert(
                message_kind,
                ReceiveTriggerMetadata {
                    component_id,
                    receive_trigger_fn: MessageRegistry::client_receive_trigger_typed::<E>,
                },
            );
    }

    /// Gather all messages present in the various `SendMessage<M>` `send_events`
    /// and buffer them in the `MessageManager`
    pub(crate) fn client_send_messages_local(
        &self,
        send_events: &mut FilteredResourcesMut,
        receive_events: &mut FilteredResourcesMut,
        sender: ClientId,
    ) {
        self.client_messages.send.iter().for_each(|metadata| {
            let send_event = send_events
                .get_mut_by_id(metadata.component_id)
                .expect("SendEvent<M> resource should be registered");
            let receive_event = receive_events
                .get_mut_by_id(
                    self.server_messages
                        .receive
                        .get(&metadata.kind)
                        .unwrap()
                        .component_id,
                )
                .expect("ReceiveEvent<M> resource should be registered");
            (metadata.send_host_server_fn)(self, send_event, receive_event, sender)
        })
    }

    /// Send a message directly from SendMessage to ReceiveMessage
    /// No need to apply entity-mapping since the client and server worlds are the same
    fn client_send_host_server_typed<M: Message>(
        &self,
        send_events: MutUntyped,
        receive_events: MutUntyped,
        sender: ClientId,
    ) {
        // SAFETY: the PtrMut corresponds to the correct resource
        let mut reader = unsafe { send_events.with_type::<Events<ClientSendMessage<M>>>() };
        let mut writer = unsafe { receive_events.with_type::<Events<ServerReceiveMessage<M>>>() };
        reader.drain().for_each(|event| {
            writer.send(ServerReceiveMessage::new(event.message, sender));
        });
    }

    /// Gather all messages present in the various `SendMessage<M>` `send_events`
    /// and buffer them in the `MessageManager`
    pub(crate) fn client_send_messages(
        &self,
        send_events: &mut FilteredResourcesMut,
        manager: &mut MessageManager,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        self.client_messages.send.iter().try_for_each(|metadata| {
            let send_events = send_events
                .get_mut_by_id(metadata.component_id)
                .expect("SendEvent<M> resource should be registered");
            (metadata.send_fn)(self, send_events, manager, entity_map)
        })
    }

    fn client_send_message_typed<M: Message>(
        &self,
        send_events: MutUntyped,
        message_manager: &mut MessageManager,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY: the PtrMut corresponds to the correct resource
        let mut reader = unsafe { send_events.with_type::<Events<ClientSendMessage<M>>>() };
        let res = reader.drain().try_for_each(|event| {

            // We write the NetworkTarget bytes, and then just concatenate the message bytes
            event.to.to_bytes(&mut message_manager.writer)?;
            self.serialize::<M>(&event.message, &mut message_manager.writer, entity_map)?;
            let message_bytes = message_manager.writer.split();
            // dbg!(&message_bytes);
            message_manager.buffer_send(message_bytes, event.channel)?;
            Ok(())
        });
        res
    }

    pub(crate) fn client_receive_message(
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
            .client_messages
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
    pub(crate) fn client_receive_message_typed<M: Message>(
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
        let mut events = unsafe { events.with_type::<Events<ClientReceiveMessage<M>>>() };
        events.send(ClientReceiveMessage::new(message, from));
        Ok(())
    }

    pub(crate) fn client_receive_trigger(
        &self,
        client_receive_events: MutUntyped,
        receive_metadata: &ReceiveTriggerMetadata,
        commands: &mut Commands,
    ) {
        (receive_metadata.receive_trigger_fn)(client_receive_events, commands);
    }

    pub(crate) fn client_receive_trigger_typed<E: Message>(
        client_receive_events: MutUntyped,
        commands: &mut Commands,
    ) {
        let mut events = unsafe {
            client_receive_events.with_type::<Events<ClientReceiveMessage<TriggerMessage<E>>>>()
        };
        events.drain().for_each(|event| {
            commands.trigger_targets(
                ClientReceiveMessage::new(event.message.event, event.from),
                event.message.target_entities,
            );
        });
    }
}

/// Trait to send remote triggers to the server
pub trait ClientTriggerExt {
    fn client_trigger<C: Channel>(&mut self, event: impl Event);

    fn client_trigger_with_targets<C: Channel>(&mut self, event: impl Event, targets: Vec<Entity>);
}

impl ClientTriggerExt for Commands<'_, '_> {
    fn client_trigger<C: Channel>(&mut self, event: impl Event) {
        self.client_trigger_with_targets::<C>(event, vec![]);
    }

    fn client_trigger_with_targets<C: Channel>(&mut self, event: impl Event, targets: Vec<Entity>) {
        self.send_event(ClientSendMessage::new::<C>(TriggerMessage {
            event,
            target_entities: targets,
        }));
    }
}

impl ClientTriggerExt for World {
    fn client_trigger<C: Channel>(&mut self, event: impl Event) {
        self.client_trigger_with_targets::<C>(event, vec![]);
    }

    fn client_trigger_with_targets<C: Channel>(&mut self, event: impl Event, targets: Vec<Entity>) {
        self.send_event(ClientSendMessage::new::<C>(TriggerMessage {
            event,
            target_entities: targets,
        }));
    }
}
