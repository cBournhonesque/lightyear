use crate::prelude::server::{is_stopped, RoomId, RoomManager, ServerError};
use crate::prelude::{
    is_host_server, Channel, ChannelKind, ClientId, MainSet, Message, MessageRegistry, MessageSend,
};
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::server::relevance::error::RelevanceError;
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
pub type ReceiveMessage<M: Message> =
    crate::shared::events::message::ReceiveMessage<M, ServerMarker>;

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
        )
            .build_state(app.world_mut())
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
        )
            .build_state(app.world_mut())
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

        let read_triggers = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .server_messages
                    .receive_trigger
                    .values()
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(read_triggers);

        app.add_systems(
            PreUpdate,
            (read_messages, read_triggers)
                .chain()
                .in_set(InternalMainSet::<ServerMarker>::ReceiveEvents)
                .run_if(not(is_stopped)),
        );
        app.add_systems(
            PostUpdate,
            (
                // we run SendEvents even if the server is stopped, so that any buffered
                // messages get drained
                send_messages
                    .in_set(InternalMainSet::<ServerMarker>::SendEvents)
                    .run_if(not(is_host_server)),
                send_messages_local
                    .in_set(InternalMainSet::<ServerMarker>::SendEvents)
                    .run_if(is_host_server),
            ),
        );
        app.configure_sets(
            PostUpdate,
            InternalMainSet::<ServerMarker>::SendEvents
                .in_set(MainSet::SendEvents)
                .before(InternalMainSet::<ServerMarker>::Send),
        );
        app.world_mut().insert_resource(message_registry);
    }
}

fn send_messages(
    mut send_events: FilteredResourcesMut,
    message_registry: ResMut<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    let _ = message_registry
        .server_send_messages(&mut send_events, connection_manager.as_mut())
        .inspect_err(|e| error!("Could not buffer message to send: {:?}", e));
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
    let _ = message_registry
        .server_send_messages_local(
            &mut server_send_events,
            &mut client_receive_events,
            connection_manager.as_mut(),
        )
        .inspect_err(|e| error!("Could not buffer message to send: {:?}", e));
}

/// Read the messages received from the clients and emit the MessageEvent events
/// Also rebroadcast the messages if needed
fn read_messages(
    mut events: FilteredResourcesMut,
    message_registry: ResMut<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        connection
            .received_messages
            .drain(..)
            .for_each(|(message_bytes, target, channel_kind)| {
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
            });
    }
}

/// Read the messages received from the clients and emit the MessageEvent events
/// Also rebroadcast the messages if needed
fn read_triggers(
    mut server_receive_events: FilteredResourcesMut,
    mut commands: Commands,
    message_registry: Res<MessageRegistry>,
) {
    message_registry
        .server_messages
        .receive_trigger
        .values()
        .for_each(|receive_metadata| {
            let events = server_receive_events
                .get_mut_by_id(receive_metadata.component_id)
                .unwrap();
            message_registry.server_receive_trigger(events, receive_metadata, &mut commands);
        })
}

impl ConnectionManager {
    /// Send a message to all clients in a room
    pub fn send_message_to_room<C: Channel, M: Message>(
        &mut self,
        message: &M,
        room_id: RoomId,
        room_manager: &RoomManager,
    ) -> Result<(), ServerError> {
        let room = room_manager
            .get_room(room_id)
            .ok_or::<ServerError>(RelevanceError::RoomIdNotFound(room_id).into())?;
        let target = NetworkTarget::Only(room.clients.iter().copied().collect());
        self.send_message_to_target::<C, M>(message, target)
    }

    /// Queues up a message to be sent to a client
    pub fn send_message<C: Channel, M: Message>(
        &mut self,
        client_id: ClientId,
        message: &M,
    ) -> Result<(), ServerError> {
        self.send_message_to_target::<C, M>(message, NetworkTarget::Single(client_id))
    }

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

    // TODO: find a way to make this work
    // /// Trigger a [`Message`] to the server using a specific [`Channel`]
    // pub fn trigger_event<C: Channel, E: Event + Message>(
    //     &mut self,
    //     event: &E,
    //     client_id: ClientId
    // ) -> Result<(), ServerError> {
    //     self.trigger_event_to_target::<C, E>(event, NetworkTarget::Single(client_id))
    // }
    //
    // /// Trigger a [`Message`] to the server using a specific [`Channel`]
    // pub fn trigger_event_to_target<C: Channel, E: Event + Message>(
    //     &mut self,
    //     event: &E,
    //     target: NetworkTarget,
    // ) -> Result<(), ServerError> {
    //     self.send_message_to_target::<C, TriggerMessage<E>>(&TriggerMessage {
    //         event: event,
    //         target_entities: vec![],
    //     }, target)
    // }
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
            self.message_registry.serialize(
                message,
                &mut self.writer,
                &mut SendEntityMap::default(),
            )?;
            let message_bytes = self.writer.split();
            self.buffer_message_bytes(message_bytes, channel_kind, target)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::server::{ReplicateToClient, ServerTriggerExt};
    use crate::prelude::{client, ClientReceiveMessage, NetworkTarget, ServerSendMessage};
    use crate::shared::message::MessageSend;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, IntegerEvent, StringMessage};
    use bevy::app::Update;
    use bevy::prelude::{EventReader, Events, Observer, ResMut, Resource, Trigger};

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

    /// System to check that we received the message on the server
    fn count_messages_observer(
        trigger: Trigger<ClientReceiveMessage<IntegerEvent>>,
        mut counter: ResMut<Counter>,
    ) {
        counter.0 += trigger.event().message.0 as usize;
    }

    /// Send a message via ConnectionManager to an external client and the local client
    #[test]
    fn server_send_message() {
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

    /// Send a message via events to an external client and the local client
    #[test]
    fn server_send_message_via_event() {
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
            .send(ServerSendMessage::new_with_target::<Channel1>(
                StringMessage("a".to_string()),
                NetworkTarget::All,
            ));

        stepper.frame_step();
        stepper.frame_step();

        // verify that the local-client received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);

        // verify that the other client received the message
        assert_eq!(stepper.client_app.world().resource::<Counter>().0, 1);
    }

    /// Send a trigger via events to an external client and the local client
    #[test]
    fn server_send_trigger_via_event() {
        let mut stepper = HostServerStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(ReplicateToClient::default())
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        stepper.server_app.init_resource::<Counter>();
        stepper.client_app.init_resource::<Counter>();
        stepper
            .server_app
            .world_mut()
            .spawn(Observer::new(count_messages_observer).with_entity(server_entity));
        stepper
            .client_app
            .world_mut()
            .spawn(Observer::new(count_messages_observer).with_entity(client_entity));

        // send a trigger from the host-server server to all clients
        stepper
            .server_app
            .world_mut()
            .server_trigger_with_targets::<Channel1>(
                IntegerEvent(10),
                NetworkTarget::All,
                vec![server_entity],
            );

        stepper.frame_step();
        stepper.frame_step();

        // verify that the local-client received the trigger
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 10);

        // verify that the other client received the trigger
        assert_eq!(stepper.client_app.world().resource::<Counter>().0, 10);
    }

    // TODO: send_trigger via ConnectionManager
}
