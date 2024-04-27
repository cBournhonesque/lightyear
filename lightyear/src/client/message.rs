//! Defines the [`ClientMessage`] enum used to send messages from the client to the server
use anyhow::Context;
use bevy::prelude::{App, EventWriter, IntoSystemConfigs, PreUpdate, Res, ResMut, Resource};
use bevy::utils::HashMap;
use bytes::Bytes;
use tracing::{error, info_span, trace};

use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};

use crate::_internal::{
    BitSerializable, ClientMarker, MessageKind, ReadBuffer, ReadWordBuffer, ServerMarker,
    WriteBuffer, WriteWordBuffer,
};
use crate::client::connection::ConnectionManager;
use crate::client::events::MessageEvent;
use crate::client::networking::is_connected;
use crate::packet::message::SingleData;
use crate::prelude::{ChannelDirection, ChannelKind, MainSet, Message, NetworkTarget};
use crate::protocol::message::MessageRegistry;
use crate::protocol::registry::NetId;

use crate::serialize::RawData;
use crate::shared::ping::message::{Ping, Pong, SyncMessage};
use crate::shared::replication::{ReplicationMessage, ReplicationMessageData};
use crate::shared::sets::InternalMainSet;

// ClientMessages can include some extra Metadata
#[derive(Encode, Decode, Clone, Debug)]
pub enum ClientMessage {
    #[bitcode_hint(frequency = 2)]
    // #[bitcode(with_serde)]
    Message(RawData, NetworkTarget),
    #[bitcode_hint(frequency = 3)]
    // #[bitcode(with_serde)]
    Replication(ReplicationMessage),
    #[bitcode_hint(frequency = 1)]
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    Ping(Ping),
    Pong(Pong),
}

/// Read the message received from the server and emit the MessageEvent event
fn read_message<M: Message>(
    message_registry: Res<MessageRegistry>,
    mut connection: ResMut<ConnectionManager>,
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
    if let Some(message_list) = connection.received_messages.remove(&net) {
        for message in message_list {
            error!("read message of type: {:?}", std::any::type_name::<M>());
            let mut reader = connection.reader_pool.start_read(&message);
            // we have to re-decode the net id
            let Ok(message) = message_registry.deserialize::<M>(
                &mut reader,
                &mut connection
                    .replication_receiver
                    .remote_entity_map
                    .remote_to_local,
            ) else {
                error!("Could not deserialize message");
                continue;
            };
            connection.reader_pool.attach(reader);
            event.send(MessageEvent::new(message, ()));
        }
    }
}

/// Register a message that can be sent from server to client
pub(crate) fn add_server_to_client_message<M: Message>(app: &mut App) {
    app.add_event::<MessageEvent<M>>();
    app.add_systems(
        PreUpdate,
        read_message::<M>
            .after(InternalMainSet::<ClientMarker>::Receive)
            .run_if(is_connected),
    );
}

impl BitSerializable for ClientMessage {
    fn encode(&self, writer: &mut WriteWordBuffer) -> anyhow::Result<()> {
        writer.encode(self, Fixed).context("could not encode")
    }
    fn decode(reader: &mut ReadWordBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        reader.decode::<Self>(Fixed).context("could not decode")
    }
}

// impl ClientMessage {
//     pub(crate) fn emit_send_logs(&self, channel_name: &str) {
//         match self {
//             ClientMessage::Message(message, _) => {
//                 let message_name = message.name();
//                 trace!(channel = ?channel_name, message = ?message_name, kind = ?message.kind(), "Sending message");
//                 #[cfg(metrics)]
//                 metrics::counter!("send_message", "channel" => channel_name, "message" => message_name).increment(1);
//             }
//             ClientMessage::Replication(message) => {
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
//             ClientMessage::Sync(message) => match message {
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
