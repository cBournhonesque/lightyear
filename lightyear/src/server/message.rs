use std::ops::DerefMut;

use bevy::app::{App, PreUpdate};
use bevy::prelude::{EventWriter, IntoSystemConfigs, Res, ResMut};
use tracing::{error, trace};

use crate::prelude::{server::is_started, Message};
use crate::protocol::message::{MessageKind, MessageRegistry};
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::server::events::ServerMessageEvent;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};

/// Read the messages received from the clients and emit the MessageEvent event
fn read_message<M: Message>(
    message_registry: Res<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
    mut event: EventWriter<ServerMessageEvent<M>>,
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
                        event.send(ServerMessageEvent::new(message, *client_id));
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

/// Register a message that can be sent from server to client
pub(crate) fn add_client_to_server_message<M: Message>(app: &mut App) {
    app.add_event::<ServerMessageEvent<M>>();
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
