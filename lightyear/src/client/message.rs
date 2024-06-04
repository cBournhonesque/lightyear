//! Defines the [`ClientMessage`] enum used to send messages from the client to the server
use bevy::prelude::{App, EventWriter, IntoSystemConfigs, PreUpdate, Res, ResMut};
use byteorder::WriteBytesExt;
use bytes::Bytes;
use tracing::error;

use crate::client::connection::ConnectionManager;
use crate::client::events::MessageEvent;
use crate::prelude::{is_connected, Message};
use crate::protocol::message::{MessageKind, MessageRegistry};
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{ClientMarker, InternalMainSet};

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
            let mut reader = Reader::from(message);
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
            .in_set(InternalMainSet::<ClientMarker>::EmitEvents)
            .run_if(is_connected),
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::writer::Writer;

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
}
