//! Defines the [`ClientMessage`] enum used to send messages from the client to the server
use bevy::prelude::{
    App, Commands, Event, EventWriter, Events, IntoSystemConfigs, PreUpdate, Res, ResMut, Trigger,
};
use byteorder::WriteBytesExt;
use bytes::Bytes;
use tracing::error;

use crate::client::connection::ConnectionManager;
use crate::client::events::MessageEvent;
use crate::connection::client::ClientConnection;
use crate::connection::server::ServerConnections;
use crate::packet::message::SendMessage;
use crate::prelude::server::ServerConfig;
use crate::prelude::{client::is_connected, is_host_server, Channel, ChannelKind, Message};
use crate::protocol::message::{MessageKind, MessageRegistry};
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::message::MessageSend;
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

#[derive(Event)]
struct SendMessageTrigger<M: Message> {
    message: Option<M>,
    channel_kind: ChannelKind,
    network_target: Option<NetworkTarget>,
}

// TODO: maybe it would be cleaner to use events even when sending messages?
//  and then have a single type-erased system that goes through all events?
//  in host-server, it would just forward them to the server?
/// In host-server mode, the client networking plugins (receive/send) are inactive,
/// so when the client sends a message to the server, we should send it directly as a
/// MessageEvent to the server
fn handle_client_to_server_message<M: Message>(
    mut trigger: Trigger<SendMessageTrigger<M>>,
    client_connection: Res<ClientConnection>,
    mut client_manager: ResMut<ConnectionManager>,
    mut server_events: Option<Events<crate::server::events::MessageEvent<M>>>,
    server_config: Option<Res<ServerConfig>>,
    server_connections: Option<Res<ServerConnections>>,
    mut server_manager: Option<ResMut<crate::server::connection::ConnectionManager>>,
) {
    let mut target = std::mem::take(&mut trigger.event_mut().network_target).unwrap();
    // if we are in host-server mode, we should send the message directly to the server
    if is_host_server(server_config, server_connections) {
        let client_id = client_connection.client.id();
        // rebroadcast the message if needed
        if trigger.event().network_target != NetworkTarget::None {
            target.exclude(NetworkTarget::Single(client_id));
            let _ = server_manager
                .unwrap()
                .erased_send_message_to_target::<M>(
                    trigger.event().message.as_ref().unwrap(),
                    trigger.event().channel_kind,
                    target,
                )
                .inspect_err(|e| {
                    error!(
                        "Could not rebroadcast host-client message to other clients: {:?}",
                        e
                    )
                });
        }
        // send the message directly to the server's Events queue
        if let Some(mut server_events) = server_events {
            // SAFETY: we know that there is a message in the event
            // We just had an option to avoid a copy.
            let message = std::mem::take(&mut trigger.event_mut().message).unwrap();
            server_events.send(crate::server::events::MessageEvent::new(message, client_id));
        }
    } else {
        // not in host-server mode, serialize and send the message as normal
        let _ = client_manager
            .erased_send_message_to_target(
                trigger.event().message.as_ref().unwrap(),
                trigger.event().channel_kind,
                target,
            )
            .inspect_err(|e| {
                error!("Could not send message to server: {:?}", e);
            });
    }
}

/// Send a message from client to server
///
/// We use a trigger `SendMessageTrigger<M>` instead of directly serializing the message
/// in case we are in host-server mode. In that situation, we just add the message directly
/// the server's Event queue.
pub(crate) fn add_client_send_message_to_server<M: Message>(app: &mut App) {
    app.observe(handle_client_to_server_message::<M>);
}

/// Register a message that can be sent from server to client
pub(crate) fn add_client_receive_message_from_server<M: Message>(app: &mut App) {
    app.add_event::<MessageEvent<M>>();
    app.add_systems(
        PreUpdate,
        read_message::<M>
            .in_set(InternalMainSet::<ClientMarker>::EmitEvents)
            .run_if(is_connected),
    );
    // send messages from client to server in host-server mode
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

// pub struct ClientToServerSendMessageCommand
//
// pub trait ClientToServerMessageCommands {
//     /// Send a [`Message`] to the server using a specific [`Channel`].
//     ///
//     /// The server will re-broadcast the message to the clients specified in the [`NetworkTarget`].
//     fn send_message_with_target<C: Channel, M: Message>(&mut self, message: &M, target: NetworkTarget);
//
//     /// Send a [`Message`] to the server using a specific [`Channel`].
//     fn send_message<C: Channel, M: Message>(&mut self);
// }
//
// impl ClientToServerMessageCommands for Commands<'_, '_> {
//     fn send_message_with_target<C: Channel, M: Message>(&mut self, message: &M, target: NetworkTarget) {
//         self.trigger(SendMessageTrigger {
//             message: Some(message),
//             channel_kind: ChannelKind::of::<C>(),
//             network_target: Some(target),
//         });
//     }
//
//     fn send_message<C: Channel, M: Message>(&mut self) {
//         todo!()
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::writer::Writer;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, Message1};

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

    #[test]
    fn client_send_message_as_host_server() {
        let mut stepper = HostServerStepper::default();
        // send a message from the local client to the server
        stepper
            .server_app
            .world_mut()
            .resource_mut::<crate::prelude::client::ConnectionManager>()
            .send_message::<Channel1, Message1>(&Message1("a".to_string()))
            .unwrap();
    }
}
