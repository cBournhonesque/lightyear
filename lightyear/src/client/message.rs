//! Defines the [`ClientMessage`] enum used to send messages from the client to the server
use crate::client::connection::ConnectionManager;
use crate::client::error::ClientError;
use crate::prelude::client::{ClientConnection, NetClient};
use crate::prelude::{client::is_connected, is_host_server, Channel, ChannelKind, ClientId, MainSet, Message, MessageRegistry, MessageSend};
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::message::private::InternalMessageSend;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{ClientMarker, InternalMainSet};
use bevy::ecs::system::{FilteredResourcesMutParamBuilder, ParamBuilder};
use bevy::prelude::*;
use byteorder::WriteBytesExt;
use bytes::Bytes;
use tracing::error;

/// Bevy [`Event`] emitted on the client when a (non-replication) message is received
#[allow(type_alias_bounds)]
pub type ReceiveMessage<M: Message> = crate::shared::events::message::ReceiveMessage<M, ClientMarker>;

#[allow(type_alias_bounds)]
pub type SendMessage<M: Message> = crate::shared::events::message::SendMessage<M, ClientMarker>;

pub struct ClientMessagePlugin;

impl Plugin for ClientMessagePlugin {
    fn build(&self, app: &mut App) {}

    /// Add the system after all messages have been added to the MessageRegistry
    fn cleanup(&self, app: &mut App) {
        // temporarily remove message_registry from the app to enable split borrows
        let message_registry = app
            .world_mut()
            .remove_resource::<MessageRegistry>()
            .unwrap();

        // Use FilteredResourceMut SystemParam to register the access dynamically to the
        // Messages in the MessageRegistry
        let send_messages = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .client_messages
                    .send
                    .iter()
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        ).build_state(app.world_mut())
            .build_system(send_messages);

        let send_messages_local = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .client_messages
                    .send
                    .iter()
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .server_messages
                    .receive
                    .values()
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            ParamBuilder,
            ParamBuilder,
        ).build_state(app.world_mut())
            .build_system(send_messages_local);

        let read_messages = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .client_messages
                    .receive
                    .values()
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(read_messages);

        app.add_systems(
            PreUpdate,
            (read_messages, read_triggers)
                .in_set(InternalMainSet::<ClientMarker>::ReceiveEvents)
                .run_if(is_connected),
        );
        app.add_systems(
            PostUpdate,
            (
                // we run SendEvents even if the client is disconnected, so that any buffered
                // messages get drained
                send_messages.in_set(InternalMainSet::<ClientMarker>::SendEvents).run_if(not(is_host_server)),
                send_messages_local.in_set(InternalMainSet::<ClientMarker>::SendEvents).run_if(is_host_server),
            )
        );
        app.configure_sets(PostUpdate, InternalMainSet::<ClientMarker>::SendEvents
            .in_set(MainSet::SendEvents)
            .before(InternalMainSet::<ClientMarker>::Send)
        );

        app.insert_resource(message_registry);
    }
}

impl ConnectionManager {
    /// Send a [`Message`] to the server using a specific [`Channel`]
    pub fn send_message<C: Channel, M: Message>(&mut self, message: &M) -> Result<(), ClientError> {
        self.send_message_to_target::<C, M>(message, NetworkTarget::None)
    }
}

impl MessageSend for ConnectionManager {}

impl InternalMessageSend for ConnectionManager {
    type Error = ClientError;

    /// Send a message to the server via a channel.
    ///
    /// The NetworkTarget will be serialized with the message, so that the server knows
    /// how to route the message to the correct target.
    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ClientError> {
        // write the target first
        // NOTE: this is ok to do because most of the time (without rebroadcast, this just adds 1 byte)
        target.to_bytes(&mut self.writer)?;
        // then write the message
        self.message_registry.serialize(
            message,
            &mut self.writer,
            &mut self.replication_receiver.remote_entity_map.local_to_remote,
        )?;
        let message_bytes = self.writer.split();

        // TODO: emit logs/metrics about the message being buffered?
        self.messages_to_send.push((message_bytes, channel_kind));
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClientMessage {
    /// Used if you want to automatically rebroadcast a message to a specific target
    pub(crate) target: NetworkTarget,
    pub(crate) message: Bytes,
}

impl ToBytes for ClientMessage {
    fn len(&self) -> usize {
        self.target.len() + self.message.len()
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        self.target.to_bytes(buffer)?;
        // NOTE: we just write the message bytes directly! We don't provide the length
        buffer.write_all(&self.message)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let target = NetworkTarget::from_bytes(buffer)?;
        // NOTE: this only works if the reader only contains the ClientMessage bytes!
        let remaining = buffer.remaining();
        let message = buffer.split_len(remaining);
        Ok(Self { message, target })
    }
}

/// Read the messages that were buffered as SendMessage<E>
/// and buffer them to be written in the various channels
fn send_messages(
    mut send_events: FilteredResourcesMut,
    commands: Commands,
    message_registry: ResMut<MessageRegistry>,
    mut connection: ResMut<ConnectionManager>,
) {
    let connection = connection.as_mut();
    let _ = message_registry.client_send_messages(
        &mut send_events,
        &mut connection.message_manager,
        &mut connection.replication_receiver.remote_entity_map.local_to_remote,
    ).inspect_err(|e| error!("Could not buffer message to send: {:?}", e));
}

/// In host-server, we read from the ClientSend and immediately write to the
/// ServerReceive events
/// TODO: handle rebroadcast
fn send_messages_local(
    mut client_send_events: FilteredResourcesMut,
    mut server_receive_events: FilteredResourcesMut,
    message_registry: ResMut<MessageRegistry>,
    client: Res<ClientConnection>,
) {
    message_registry.client_send_messages_local(
        &mut client_send_events,
        &mut server_receive_events,
        client.id(),
    );
}

/// Read the messages received from the server and handle them:
/// - Messages: send a MessageEvent
/// - Events: send them to EventWriter or trigger them
fn read_messages(
    mut events: FilteredResourcesMut,
    message_registry: ResMut<MessageRegistry>,
    mut connection: ResMut<ConnectionManager>,
) {
    // TODO: we could completely split out the MessageManager out of the connection manager!
    // enable split-borrows
    let connection = connection.as_mut();
    // TODO: switch to directly reading from the message manager!
    //     connection
    //         .message_manager
    //         .channels
    //         .iter_mut()
    //         // TODO: separate internal from external channels in MessageManager?
    //         .filter(|(kind, _)| {
    //             **kind != ChannelKind::of::<PingChannel>()
    //             && **kind != ChannelKind::of::<PongChannel>()
    //             && **kind != ChannelKind::of::<EntityActionsChannel>()
    //             && **kind != ChannelKind::of::<EntityUpdatesChannel>()
    //             && **kind != ChannelKind::of::<InputChannel>()
    //         })
    connection
        .received_messages
        .drain(..)
        .for_each(|(_, bytes)| {
            let _ = message_registry
                // we have to re-decode the net id
                .client_receive_message(
                    &mut events,
                    // TODO: include the client that rebroadcasted the message?
                    ClientId::Local(0),
                    &mut Reader::from(bytes),
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                )
                .inspect_err(|e| error!("Could not deserialize message: {:?}", e));
        });
}

/// Read the messages received from the server and handle them:
/// - Messages: send a MessageEvent
/// - Events: send them to EventWriter or trigger them
fn read_triggers(
    mut commands: Commands,
    message_registry: ResMut<MessageRegistry>,
    mut connection: ResMut<ConnectionManager>,
) {
    // TODO: we could completely split out the MessageManager out of the connection manager!
    // enable split-borrows
    let connection = connection.as_mut();
    // TODO: switch to directly reading from the message manager!
    //     connection
    //         .message_manager
    //         .channels
    //         .iter_mut()
    //         // TODO: separate internal from external channels in MessageManager?
    //         .filter(|(kind, _)| {
    //             **kind != ChannelKind::of::<PingChannel>()
    //             && **kind != ChannelKind::of::<PongChannel>()
    //             && **kind != ChannelKind::of::<EntityActionsChannel>()
    //             && **kind != ChannelKind::of::<EntityUpdatesChannel>()
    //             && **kind != ChannelKind::of::<InputChannel>()
    //         })
    connection
        .received_triggers
        .drain(..)
        .for_each(|(_, bytes)| {
            let _ = message_registry
                // we have to re-decode the net id
                .client_receive_trigger(
                    &mut commands,
                    &mut Reader::from(bytes),
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                )
                .inspect_err(|e| error!("Could not deserialize message: {:?}", e));
        });
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::client::ClientTriggerExt;
    use crate::prelude::{ClientSendMessage, ServerReceiveMessage};
    use crate::serialize::writer::Writer;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, IntegerEvent, StringMessage};
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::{EventReader, Resource, Update};

    #[test]
    fn client_message_serde() {
        let data = ClientMessage {
            target: NetworkTarget::None,
            message: Bytes::from_static(b"hello world"),
        };
        let mut writer = Writer::default();
        data.to_bytes(&mut writer).unwrap();
        let bytes = writer.to_bytes();

        let mut reader = Reader::from(bytes);
        let result = ClientMessage::from_bytes(&mut reader).unwrap();
        assert_eq!(data, result);
    }

    #[derive(Resource, Default)]
    struct Counter(usize);

    /// System to check that we received the message on the server
    fn count_messages(
        mut counter: ResMut<Counter>,
        mut events: EventReader<ServerReceiveMessage<StringMessage>>,
    ) {
        for event in events.read() {
            assert_eq!(event.message().0, "a".to_string());
            counter.0 += 1;
        }
    }
    /// System to check that we received the message on the server
    fn count_messages_observer(
        trigger: Trigger<ServerReceiveMessage<IntegerEvent>>,
        mut counter: ResMut<Counter>,
    ) {
        counter.0 += trigger.event().message.0 as usize;
    }

    #[test]
    fn client_send_message_as_host_server_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);

        // send a message from the local client to the server
        stepper
            .server_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .send_message::<Channel1, StringMessage>(&StringMessage("a".to_string()))
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);
    }

    #[test]
    fn client_send_message_via_send_event() {
        let mut stepper = BevyStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);

        // Send the message by writing to the SendMessage<M> Events
        stepper.client_app.world_mut().resource_mut::<Events<ClientSendMessage<StringMessage>>>()
            .send(ClientSendMessage::new::<Channel1>(StringMessage("a".to_string())));

        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);
    }

    #[test]
    fn client_send_message_via_send_event_as_host_server() {
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);

        // Send the message by writing to the SendMessage<M> Events
        stepper.server_app.world_mut().resource_mut::<Events<ClientSendMessage<StringMessage>>>()
            .send(ClientSendMessage::new::<Channel1>(StringMessage("a".to_string())));

        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);
    }

    #[test]
    fn client_send_trigger_via_send_event() {
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_observer(count_messages_observer);

        // Send the message by writing to the SendMessage<M> Events
        stepper.client_app.world_mut().client_trigger::<Channel1>(IntegerEvent(10));

        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 10);
    }
}

