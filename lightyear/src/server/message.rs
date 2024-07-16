use std::ops::DerefMut;

use crate::prelude::{server::is_started, Message};
use crate::protocol::message::{MessageKind, MessageRegistry};
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::server::events::MessageEvent;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use bevy::app::{App, PreUpdate};
use bevy::prelude::{EventWriter, IntoSystemConfigs, Res, ResMut};
use tracing::{error, trace};

/// Read the messages received from the clients and emit the MessageEvent event
fn read_message<M: Message>(
    message_registry: Res<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
    mut event: EventWriter<MessageEvent<M>>,
) {
    let kind = MessageKind::of::<M>();
    let Some(net) = message_registry.kind_map.net_id(&kind).copied() else {
        error!(
            "Could not find the network id for the message kind: {:?}",
            kind
        );
        return;
    };
    // re-borrow to allow split borrows
    let connection_manager = connection_manager.deref_mut();
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        if let Some(message_list) = connection.received_messages.remove(&net) {
            for (message_bytes, target, channel_kind) in message_list {
                let mut reader = Reader::from(message_bytes);
                match message_registry.deserialize::<M>(
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(message) => {
                        // rebroadcast
                        if target != NetworkTarget::None {
                            connection.messages_to_rebroadcast.push((
                                reader.consume(),
                                target,
                                channel_kind,
                            ));
                        }
                        event.send(MessageEvent::new(message, *client_id));
                        trace!("Received message: {:?}", std::any::type_name::<M>());
                    }
                    Err(e) => {
                        error!(
                            "Could not deserialize message {}: {:?}",
                            std::any::type_name::<M>(),
                            e
                        );
                    }
                }
            }
        }
    }
}

// /// Observer Trigger to send a message.
// ///
// /// If we are running in host-server mode, the messages that are destined to the local client will be
// /// sent directly to the client's Event queue.
// #[derive(Event)]
// struct SendMessageTrigger<M: Message> {
//     message: Option<M>,
//     channel_kind: ChannelKind,
//     network_target: Option<NetworkTarget>,
// }
//
// // TODO: maybe it would be cleaner to use events even when sending messages?
// //  and then have a single type-erased system that goes through all events?
// //  in host-server, it would just forward them to the server?
// /// In host-server mode, the client networking plugins (receive/send) are inactive,
// /// so when the client sends a message to the server, we should send it directly as a
// /// MessageEvent to the server
// fn handle_server_send_message_to_client<M: Message>(
//     mut trigger: Trigger<SendMessageTrigger<M>>,
//     client_connection: Option<Res<ClientConnection>>,
//     mut client_events: Option<Events<crate::client::events::MessageEvent<M>>>,
//     server_config: Option<Res<ServerConfig>>,
//     server_connections: Option<Res<ServerConnections>>,
//     mut server_manager: ResMut<ConnectionManager>,
// ) {
//     let mut target = std::mem::take(&mut trigger.event_mut().network_target).unwrap();
//     // if we are in host-server mode, the messages destined to the local client should be
//     // sent directly to the client's Events queue
//     if is_host_server(server_config, server_connections) {
//         let client_id = client_connection
//             .expect("We are running in host-server mode but could not access the client connection")
//             .client
//             .id();
//         let send_to_local_client = target.targets(&client_id);
//         target.exclude(NetworkTarget::Single(client_id));
//         // send the message normally to other clients
//         if !target.is_empty() {
//             let _ = server_manager
//                 .erased_send_message_to_target::<M>(
//                     trigger.event().message.as_ref().unwrap(),
//                     trigger.event().channel_kind,
//                     target,
//                 )
//                 .inspect_err(|e| {
//                     error!(
//                         "Could not rebroadcast host-client message to other clients: {:?}",
//                         e
//                     )
//                 });
//         }
//         // send the message directly to the local client's Events queue
//         if let Some(mut client_events) = client_events {
//             // SAFETY: we know that there is a message in the event
//             // We just had an option to avoid a copy.
//             let message = std::mem::take(&mut trigger.event_mut().message).unwrap();
//             client_events.send(crate::client::events::MessageEvent::new(message, ()));
//         }
//     } else {
//         // not in host-server mode, serialize and send the message as normal
//         let _ = server_manager
//             .erased_send_message_to_target(
//                 trigger.event().message.as_ref().unwrap(),
//                 trigger.event().channel_kind,
//                 target,
//             )
//             .inspect_err(|e| {
//                 error!("Could not send message to clients: {:?}", e);
//             });
//     }
// }
//
// /// Send a message from server to clients
// ///
// /// We use a trigger `SendMessageTrigger<M>` instead of directly serializing the message
// /// in case we are in host-server mode. In that situation, we just add the message directly
// /// the local client's Event queue.
// pub(crate) fn add_server_send_message_to_client<M: Message>(app: &mut App) {
//     app.observe(handle_server_send_message_to_client::<M>);
// }

/// Register a message that can be sent from client to server
pub(crate) fn add_server_receive_message_from_client<M: Message>(app: &mut App) {
    app.add_event::<MessageEvent<M>>();
    app.add_systems(
        PreUpdate,
        read_message::<M>
            .in_set(InternalMainSet::<ServerMarker>::EmitEvents)
            .run_if(is_started),
    );
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
