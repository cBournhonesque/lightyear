use crate::prelude::server::{is_stopped, ServerError};
use crate::prelude::{is_host_server, ChannelKind, MainSet, Message, MessageRegistry, MessageSend};
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::shared::message::private::InternalMessageSend;
use crate::shared::replication::entity_map::SendEntityMap;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use bevy::ecs::system::{FilteredResourcesMutParamBuilder, ParamBuilder};
use bevy::prelude::*;
use bytes::Bytes;
use tracing::error;

/// Bevy [`Event`] emitted on the server on the frame where a (non-replication) message is received
#[allow(type_alias_bounds)]
pub type ReceiveMessage<M: Message> = crate::shared::events::message::ReceiveMessage<M, ServerMarker>;

#[allow(type_alias_bounds)]
pub type SendMessage<M: Message> = crate::shared::events::message::SendMessage<M, ServerMarker>;

/// Plugin that adds functionality related to receiving messages from clients
#[derive(Default)]
pub struct ServerMessagePlugin;

impl Plugin for ServerMessagePlugin {
    fn build(&self, app: &mut App) {}

    /// Add the system after all messages have been added to the MessageRegistry
    fn cleanup(&self, app: &mut App) {
        let message_registry = app
            .world_mut()
            .remove_resource::<MessageRegistry>()
            .unwrap();
        // Use FilteredResourceMut SystemParam to register the access dynamically to the
        // Messages in the MessageRegistry
        let send_messages = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .server_messages
                    .send
                    .iter()
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            ParamBuilder,
            ParamBuilder,
        ).build_state(app.world_mut())
            .build_system(send_messages);

        let send_messages_local = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .server_messages
                    .send
                    .iter()
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
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
        ).build_state(app.world_mut())
            .build_system(send_messages_local);

        let read_messages = (
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
        )
            .build_state(app.world_mut())
            .build_system(read_messages);

        app.add_systems(
            PreUpdate,
            (read_messages, read_triggers)
                .in_set(InternalMainSet::<ServerMarker>::ReceiveEvents)
                .run_if(not(is_stopped)),
        );
        app.add_systems(
            PostUpdate,
            (
                // we run SendEvents even if the server is stopped, so that any buffered
                // messages get drained
                send_messages.in_set(InternalMainSet::<ServerMarker>::SendEvents).run_if(not(is_host_server)),
                send_messages_local.in_set(InternalMainSet::<ServerMarker>::SendEvents).run_if(is_host_server),
            )
        );
        app.configure_sets(PostUpdate, InternalMainSet::<ServerMarker>::SendEvents
            .in_set(MainSet::SendEvents)
            .before(InternalMainSet::<ServerMarker>::Send)
        );
        app.world_mut().insert_resource(message_registry);
    }
}

fn send_messages(
    mut send_events: FilteredResourcesMut,
    message_registry: ResMut<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    let _ = message_registry.server_send_messages(
        &mut send_events,
        connection_manager.as_mut()
    ).inspect_err(|e| error!("Could not buffer message to send: {:?}", e));
}

/// In host-server, we read from the ServerSend and immediately write to the
/// ClientReceive events
/// TODO: handle rebroadcast
fn send_messages_local(
    mut server_send_events: FilteredResourcesMut,
    mut client_receive_events: FilteredResourcesMut,
    message_registry: ResMut<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    let _ = message_registry.server_send_messages_local(
        &mut server_send_events,
        &mut client_receive_events,
        connection_manager.as_mut()
    ).inspect_err(|e| error!("Could not buffer message to send: {:?}", e));
}

/// Read the messages received from the clients and emit the MessageEvent events
/// Also rebroadcast the messages if needed
fn read_messages(
    mut events: FilteredResourcesMut,
    message_registry: ResMut<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        connection.received_messages.drain(..).for_each(
            |(message_bytes, target, channel_kind)| {
                let mut reader = Reader::from(message_bytes);
                match message_registry.server_receive_message(
                    &mut events,
                    *client_id,
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(_) => {
                        // rebroadcast
                        if target != NetworkTarget::None {
                            connection.messages_to_rebroadcast.push((
                                reader.consume(),
                                target,
                                channel_kind,
                            ));
                        }
                    }
                    Err(e) => {
                        error!("Could not deserialize message: {e:?}");
                    }
                }
            },
        );
    }
}

/// Read the messages received from the clients and emit the MessageEvent events
/// Also rebroadcast the messages if needed
fn read_triggers(
    mut commands: Commands,
    message_registry: ResMut<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        connection.received_triggers.drain(..).for_each(
            |(message_bytes, target, channel_kind)| {
                let mut reader = Reader::from(message_bytes);
                match message_registry.server_receive_trigger(
                    &mut commands,
                    &mut reader,
                    *client_id,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(_) => {
                        // rebroadcast
                        if target != NetworkTarget::None {
                            connection.messages_to_rebroadcast.push((
                                reader.consume(),
                                target,
                                channel_kind,
                            ));
                        }
                    }
                    Err(e) => {
                        error!("Could not deserialize message: {e:?}");
                    }
                }
            },
        );
    }
}

impl ConnectionManager {
    pub(crate) fn buffer_message_bytes(
        &mut self,
        message: Bytes,
        channel: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        self.connections
            .iter_mut()
            .filter(|(id, _)| target.targets(id))
            .try_for_each(|(_, c)| {
                // for local clients, we don't want to buffer messages in the MessageManager since
                // there is no io
                if c.is_local_client() {
                    c.local_messages_to_send.push(message.clone())
                } else {
                    // NOTE: this clone is O(1), it just increments the reference count
                    c.buffer_message(message.clone(), channel)?;
                }
                Ok::<(), ServerError>(())
            })
    }


    /// Buffer a `MapEntities` message to remote clients.
    /// We cannot serialize the message once, we need to instead map the message for each client
    /// using the `EntityMap` of that connection.
    pub(crate) fn buffer_map_entities_message<M: Message>(
        &mut self,
        message: &M,
        channel: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        self.connections
            .iter_mut()
            .filter(|(id, _)| target.targets(id))
            .try_for_each(|(_, c)| {
                self.message_registry.serialize(
                    message,
                    &mut self.writer,
                    &mut c.replication_receiver.remote_entity_map.local_to_remote,
                )?;
                let message_bytes = self.writer.split();
                // for local clients, we don't want to buffer messages in the MessageManager since
                // there is no io
                if c.is_local_client() {
                    c.local_messages_to_send.push(message_bytes);
                } else {
                    c.buffer_message(message_bytes, channel)?;
                }
                Ok::<(), ServerError>(())
            })
    }
}

impl MessageSend for ConnectionManager {}

impl InternalMessageSend for ConnectionManager {
    type Error = ServerError;

    /// Serialize the message and buffer it to be sent in each `Connection`.
    ///
    /// - If the message is not `MapEntities`, we can serialize it once and reuse the same bytes
    ///   for all `Connections`.
    /// - If it is `MapEntities`, we need to map it in each connection.
    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> Result<(), ServerError> {
        if self.message_registry.is_map_entities::<M>() {
            self.buffer_map_entities_message(message, channel_kind, target)?;
        } else {
            self.message_registry
                .serialize(message, &mut self.writer, &mut SendEntityMap::default())?;
            let message_bytes = self.writer.split();
            self.buffer_message_bytes(message_bytes, channel_kind, target)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::{ClientReceiveMessage, NetworkTarget, ServerSendMessage};
    use crate::shared::message::MessageSend;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, StringMessage};
    use crate::tests::stepper::BevyStepper;
    use bevy::app::Update;
    use bevy::prelude::{EventReader, Events, ResMut, Resource};

    #[derive(Resource, Default)]
    struct Counter(usize);

    /// System to check that we received the message on the server
    fn count_messages(
        mut counter: ResMut<Counter>,
        mut events: EventReader<ClientReceiveMessage<StringMessage>>,
    ) {
        for event in events.read() {
            assert_eq!(event.message().0, "a".to_string());
            counter.0 += 1;
        }
    }

    /// Send a message via ConnectionManager to an external client and the local client
    #[test]
    fn server_send_message_to_local_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.client_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);
        stepper.client_app.add_systems(Update, count_messages);

        // send a message from the host-server server to all clients
        stepper
            .server_app
            .world_mut()
            .resource_mut::<crate::prelude::server::ConnectionManager>()
            .send_message_to_target::<Channel1, StringMessage>(
                &StringMessage("a".to_string()),
                NetworkTarget::All,
            )
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the local-client received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);

        // verify that the other client received the message
        assert_eq!(stepper.client_app.world().resource::<Counter>().0, 1);
    }

    /// Send a message via event
    #[test]
    fn server_send_message_via_send_event() {
        let mut stepper = BevyStepper::default();

        stepper.client_app.init_resource::<Counter>();
        stepper.client_app.add_systems(Update, count_messages);

        // Send the message by writing to the SendMessage<M> Events
        stepper.server_app.world_mut().resource_mut::<Events<ServerSendMessage<StringMessage>>>()
            .send(ServerSendMessage::new_with_target::<Channel1>(StringMessage("a".to_string()), NetworkTarget::All));

        stepper.frame_step();
        stepper.frame_step();

        // verify that the client received the message
        assert_eq!(stepper.client_app.world().resource::<Counter>().0, 1);
    }

    /// Send a message via events to an external client and the local client
    #[test]
    fn server_send_message_via_send_event_local() {
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.client_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);
        stepper.client_app.add_systems(Update, count_messages);

        // send a message from the host-server server to all clients
        stepper
            .server_app
            .world_mut()
            .resource_mut::<Events<ServerSendMessage<StringMessage>>>()
            .send(ServerSendMessage::new_with_target::<Channel1>(StringMessage("a".to_string()), NetworkTarget::All));

        stepper.frame_step();
        stepper.frame_step();

        // verify that the local-client received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);

        // verify that the other client received the message
        assert_eq!(stepper.client_app.world().resource::<Counter>().0, 1);
    }
}

