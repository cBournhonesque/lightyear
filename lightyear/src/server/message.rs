use std::ops::DerefMut;

use anyhow::Context;
use bevy::app::{App, PreUpdate};
use bevy::prelude::{EventWriter, IntoSystemConfigs, Res, ResMut, Resource};
use bevy::utils::HashMap;
use bytes::Bytes;
use tracing::{error, info_span, trace};

use bitcode::__private::Fixed;
use bitcode::{Decode, Encode};

use crate::_internal::{BitSerializable, MessageKind, ServerMarker};
use crate::packet::message::SingleData;
use crate::prelude::{MainSet, Message, NetworkTarget};
use crate::protocol::message::MessageRegistry;
use crate::protocol::registry::NetId;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::serialize::RawData;
use crate::server::connection::ConnectionManager;
use crate::server::events::MessageEvent;
use crate::server::networking::is_started;
use crate::shared::ping::message::{Ping, Pong, SyncMessage};
use crate::shared::replication::{ReplicationMessage, ReplicationMessageData};
use crate::shared::sets::InternalMainSet;

#[derive(Encode, Decode, Clone, Debug)]
pub enum ServerMessage {
    #[bitcode_hint(frequency = 2)]
    // #[bitcode(with_serde)]
    Message(RawData),
    #[bitcode_hint(frequency = 3)]
    // #[bitcode(with_serde)]
    Replication(ReplicationMessage),
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    #[bitcode_hint(frequency = 1)]
    Ping(Ping),
    #[bitcode_hint(frequency = 1)]
    Pong(Pong),
}

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
                let mut reader = connection.reader_pool.start_read(&message_bytes);
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
                            if let Ok(message_bytes) =
                                message_registry.serialize(&message, &mut connection_manager.writer)
                            {
                                connection.messages_to_rebroadcast.push((
                                    message_bytes,
                                    target,
                                    channel_kind,
                                ));
                            }
                        }
                        event.send(MessageEvent::new(message, *client_id));
                    }
                    Err(e) => {
                        error!(
                            "Could not deserialize message {}: {:?}",
                            std::any::type_name::<M>(),
                            e
                        );
                    }
                }
                connection.reader_pool.attach(reader);
            }
        }
    }
}

/// Register a message that can be sent from server to client
pub(crate) fn add_client_to_server_message<M: Message>(app: &mut App) {
    app.add_event::<MessageEvent<M>>();
    app.add_systems(
        PreUpdate,
        read_message::<M>
            .after(InternalMainSet::<ServerMarker>::Receive)
            .run_if(is_started),
    );
}

impl BitSerializable for ServerMessage {
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        writer.encode(self, Fixed).context("could not encode")
    }
    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        reader.decode::<Self>(Fixed).context("could not decode")
    }
}

//
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
