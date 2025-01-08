use crate::prelude::server::{is_stopped, RoomId, RoomManager, ServerError};
use crate::prelude::{Channel, ChannelKind, ClientId, Message};
use crate::protocol::message::MessageRegistry;
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::server::relevance::error::RelevanceError;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use bevy::app::{App, Plugin, PreUpdate};
use bevy::prelude::{not, Commands, IntoSystemConfigs, Mut, ResMut, World};
use tracing::{error, trace};

/// Plugin that adds functionality related to receiving messages from clients
#[derive(Default)]
pub struct ServerMessagePlugin;

impl Plugin for ServerMessagePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            read_messages
                .in_set(InternalMainSet::<ServerMarker>::EmitEvents)
                .run_if(not(is_stopped)),
        );
    }
}

/// Read the messages received from the clients and emit the MessageEvent events
/// Also rebroadcast the messages if needed
fn read_messages(mut commands: Commands, mut connection_manager: ResMut<ConnectionManager>) {
    // re-borrow to allow split borrows
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        connection
            .received_messages
            .iter_mut()
            .for_each(|(net_id, message_list)| {
                message_list
                    .drain(..)
                    .for_each(|(message_bytes, target, channel_kind)| {
                        let mut reader = Reader::from(message_bytes);
                        // make copies to avoid `connection_manager` to be moved inside the closure
                        let net_id = *net_id;
                        let client_id = *client_id;
                        commands.queue(move |world: &mut World| {
                            // NOTE: removing the resources is a bit risky... however we use the world
                            // only to get the Events<MessageEvent<M>> so it should be ok
                            world.resource_scope(|world, registry: Mut<MessageRegistry>| {
                                world.resource_scope(
                                    |world, mut manager: Mut<ConnectionManager>| {
                                            let connection =
                                                manager.connection_mut(client_id).unwrap();
                                            match registry.receive_message(
                                                net_id,
                                                world,
                                                client_id,
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
                                                    trace!("Received message! NetId: {net_id:?}");
                                                }
                                                Err(e) => {
                                                    error!("Could not deserialize message (NetId: {net_id:?}): {e:?}");
                                                }
                                            }
                                    },
                                )
                            });
                        })
                    })
            });
    }
}

pub trait ServerMessageExt: crate::shared::message::private::InternalMessageSend {
    fn send_message_to_client<C: Channel, M: Message>(&mut self, message: &M, client_id: ClientId) {
        self.send_message_to_target::<C, M>(message, NetworkTarget::Single(client_id))
    }

    /// Send a message to all clients in a room
    fn send_message_to_room<C: Channel, M: Message>(&mut self, message: &M, room_id: RoomId);

    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }
}

impl ServerMessageExt for Commands {
    fn send_message_to_room<C: Channel, M: Message>(&mut self, message: &M, room_id: RoomId) {
        // TODO: avoid code duplication by creating Command structs which can be combined
        self.queue(|world: &mut World| {
            world.commands().
            let Some(room_manager) = world.get_resource::<RoomManager>() else {
                return;
                // return Err(ConnectionError::NotFound.into());
            };
            let Some(mut manager) = world.get_resource_mut::<ConnectionManager>() else {
                return;
                // return Err(ConnectionError::NotFound.into());
            };
            let Some(registry) = world.get_resource::<MessageRegistry>() else {
                return;
                // return Err(ConnectionError::NotFound.into());
            };
            let room = room_manager
                .get_room(room_id)
                .ok_or::<ServerError>(RelevanceError::RoomIdNotFound(room_id).into())?;
            let target = NetworkTarget::Only(room.clients.iter().copied().collect());

            if registry.is_map_entities::<M>() {
                manager.buffer_map_entities_message(message, ChannelKind::of::<C>(), target)?;
            } else {
                registry.serialize(message, &mut manager.writer, None)?;
                let message_bytes = manager.writer.split();
                manager.buffer_message_bytes(message_bytes, ChannelKind::of::<C>(), target)?;
            }
        });
    }
}

impl crate::shared::message::private::InternalMessageSend for Commands {
    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) {
        self.queue(|world: &mut World| {
            // TODO: fetch the entity that contains the Transport/Writer/MessageManager
            let Some(mut manager) = world.get_resource_mut::<ConnectionManager>() else {
                return;
                // return Err(ConnectionError::NotFound.into());
            };
            let Some(registry) = world.get_resource::<MessageRegistry>() else {
                return;
                // return Err(ConnectionError::NotFound.into());
            };
            if registry.is_map_entities::<M>() {
                manager.buffer_map_entities_message(message, channel_kind, target)?;
            } else {
                registry.serialize(message, &mut manager.writer, None)?;
                let message_bytes = manager.writer.split();
                manager.buffer_message_bytes(message_bytes, channel_kind, target)?;
            }
        });
    }
}

// impl ServerMessage {
//     pub(crate) fn emit_send_logs(&self, channel_name: &str) {
//         match self {
//             ServerMessage::Message(message) => {
//                 let message_name = message.name();
//                 trace!(channel = ?channel_name, message = ?message_name, kind = ?message.kind(), "Sending message");
//                 #[cfg(metrics)]
//                 metrics::counter!("send_message", "channel" => channel_name, "message" => message_name).increment(1);
//             }
//             ServerMessage::Replication(message) => {
//                 let _span = info_span!("send replication message", channel = ?channel_name, group_id = ?message.group_id);
//                 #[cfg(metrics)]
//                 metrics::counter!("send_replication_actions").increment(1);
//                 match &message.data {
//                     ReplicationMessageData::Actions(m) => {
//                         for (entity, actions) in &m.actions {
//                             let _span = info_span!("send replication actions", ?entity);
//                             if actions.spawn {
//                                 trace!("Send entity spawn");
//                                 #[cfg(metrics)]
//                                 metrics::counter!("send_entity_spawn").increment(1);
//                             }
//                             if actions.despawn {
//                                 trace!("Send entity despawn");
//                                 #[cfg(metrics)]
//                                 metrics::counter!("send_entity_despawn").increment(1);
//                             }
//                             if !actions.insert.is_empty() {
//                                 let components = actions
//                                     .insert
//                                     .iter()
//                                     .map(|c| c.into())
//                                     .collect::<Vec<P::ComponentKinds>>();
//                                 trace!(?components, "Sending component insert");
//                                 #[cfg(metrics)]
//                                 {
//                                     for component in components {
//                                         metrics::counter!("send_component_insert", "component" => kind).increment(1);
//                                     }
//                                 }
//                             }
//                             if !actions.remove.is_empty() {
//                                 trace!(?actions.remove, "Sending component remove");
//                                 #[cfg(metrics)]
//                                 {
//                                     for kind in actions.remove {
//                                         metrics::counter!("send_component_remove", "component" => kind).increment(1);
//                                     }
//                                 }
//                             }
//                             if !actions.updates.is_empty() {
//                                 let components = actions
//                                     .updates
//                                     .iter()
//                                     .map(|c| c.into())
//                                     .collect::<Vec<P::ComponentKinds>>();
//                                 trace!(?components, "Sending component update");
//                                 #[cfg(metrics)]
//                                 {
//                                     for component in components {
//                                         metrics::counter!("send_component_update", "component" => kind).increment(1);
//                                     }
//                                 }
//                             }
//                         }
//                     }
//                     ReplicationMessageData::Updates(m) => {
//                         for (entity, updates) in &m.updates {
//                             let _span = info_span!("send replication updates", ?entity);
//                             let components = updates
//                                 .iter()
//                                 .map(|c| c.into())
//                                 .collect::<Vec<P::ComponentKinds>>();
//                             trace!(?components, "Sending component update");
//                             #[cfg(metrics)]
//                             {
//                                 for component in components {
//                                     metrics::counter!("send_component_update", "component" => kind)
//                                         .increment(1);
//                                 }
//                             }
//                         }
//                     }
//                 }
//             }
//             ServerMessage::Sync(message) => match message {
//                 SyncMessage::Ping(_) => {
//                     trace!(channel = ?channel_name, "Sending ping");
//                     #[cfg(metrics)]
//                     metrics::counter!("send_ping", "channel" => channel_name).increment(1);
//                 }
//                 SyncMessage::Pong(_) => {
//                     trace!(channel = ?channel_name, "Sending pong");
//                     #[cfg(metrics)]
//                     metrics::counter!("send_pong", "channel" => channel_name).increment(1);
//                 }
//             },
//         }
//     }
// }

// TODO: another option is to add ClientMessage and ServerMessage to ProtocolMessage
// then we can keep the shared logic in connection.mod. We just lose 1 bit everytime...

#[cfg(test)]
mod tests {
    use crate::prelude::NetworkTarget;
    use crate::shared::message::MessageSend;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, StringMessage};
    use bevy::app::Update;
    use bevy::prelude::{EventReader, ResMut, Resource};

    #[derive(Resource, Default)]
    struct Counter(usize);

    /// System to check that we received the message on the server
    fn count_messages(
        mut counter: ResMut<Counter>,
        mut events: EventReader<crate::client::events::MessageEvent<StringMessage>>,
    ) {
        for event in events.read() {
            assert_eq!(event.message().0, "a".to_string());
            counter.0 += 1;
        }
    }

    /// In host-server mode, the server is sending a message to the local client
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
        stepper.frame_step();
        stepper.frame_step();

        // verify that the local-client received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);

        // verify that the other client received the message
        assert_eq!(stepper.client_app.world().resource::<Counter>().0, 1);
    }
}
