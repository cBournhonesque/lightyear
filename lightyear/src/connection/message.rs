/*!
Defines the [`ClientMessage`] and [`ServerMessage`] enums that are used to send messages over the network
*/
use crate::prelude::{ChannelKind, NetworkTarget};
use serde::{Deserialize, Serialize};
use tracing::{info_span, trace};

use crate::protocol::Protocol;
use crate::shared::ping::message::SyncMessage;
use crate::shared::replication::{ReplicationMessage, ReplicationMessageData};
use crate::utils::named::Named;

pub(crate) struct MessageMetadata {
    pub(crate) target: NetworkTarget,
    pub(crate) channel: ChannelKind,
}

// ClientMessages can include some extra Metadata
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ClientMessage<P: Protocol> {
    Message(P::Message, NetworkTarget),
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    Sync(SyncMessage),
}

impl<P: Protocol> ClientMessage<P> {
    pub(crate) fn emit_send_logs(&self, channel_name: &str) {
        match self {
            ClientMessage::Message(message, _) => {
                let message_name = message.name();
                trace!(channel = ?channel_name, message = ?message_name, "Sending message");
                #[cfg(metrics)]
                metrics::increment_counter!("send_message", "channel" => channel_name, "message" => message_name);
            }
            ClientMessage::Replication(message) => {
                let _span = info_span!("send replication message", channel = ?channel_name, group_id = ?message.group_id);
                #[cfg(metrics)]
                metrics::increment_counter!("send_replication_actions");
                match &message.data {
                    ReplicationMessageData::Actions(m) => {
                        for (entity, actions) in &m.actions {
                            let _span = info_span!("send replication actions", ?entity);
                            if actions.spawn {
                                trace!("Send entity spawn");
                                #[cfg(metrics)]
                                metrics::increment_counter!("send_entity_spawn");
                            }
                            if actions.despawn {
                                trace!("Send entity despawn");
                                #[cfg(metrics)]
                                metrics::increment_counter!("send_entity_despawn");
                            }
                            if !actions.insert.is_empty() {
                                let components = actions
                                    .insert
                                    .iter()
                                    .map(|c| c.into())
                                    .collect::<Vec<P::ComponentKinds>>();
                                trace!(?components, "Sending component insert");
                                #[cfg(metrics)]
                                {
                                    for component in components {
                                        metrics::increment_counter!("send_component_insert", "component" => kind);
                                    }
                                }
                            }
                            if !actions.remove.is_empty() {
                                trace!(?actions.remove, "Sending component remove");
                                #[cfg(metrics)]
                                {
                                    for kind in actions.remove {
                                        metrics::increment_counter!("send_component_remove", "component" => kind);
                                    }
                                }
                            }
                            if !actions.updates.is_empty() {
                                let components = actions
                                    .updates
                                    .iter()
                                    .map(|c| c.into())
                                    .collect::<Vec<P::ComponentKinds>>();
                                trace!(?components, "Sending component update");
                                #[cfg(metrics)]
                                {
                                    for component in components {
                                        metrics::increment_counter!("send_component_update", "component" => kind);
                                    }
                                }
                            }
                        }
                    }
                    ReplicationMessageData::Updates(m) => {
                        for (entity, updates) in &m.updates {
                            let _span = info_span!("send replication updates", ?entity);
                            let components = updates
                                .iter()
                                .map(|c| c.into())
                                .collect::<Vec<P::ComponentKinds>>();
                            trace!(?components, "Sending component update");
                            #[cfg(metrics)]
                            {
                                for component in components {
                                    metrics::increment_counter!("send_component_update", "component" => kind);
                                }
                            }
                        }
                    }
                }
            }
            ClientMessage::Sync(message) => match message {
                SyncMessage::Ping(_) => {
                    trace!(channel = ?channel_name, "Sending ping");
                    #[cfg(metrics)]
                    metrics::increment_counter!("send_ping", "channel" => channel_name);
                }
                SyncMessage::Pong(_) => {
                    trace!(channel = ?channel_name, "Sending pong");
                    #[cfg(metrics)]
                    metrics::increment_counter!("send_pong", "channel" => channel_name);
                }
            },
        }
    }
}

// TODO: add frequency information
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ServerMessage<P: Protocol> {
    Message(P::Message),
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    Sync(SyncMessage),
}

impl<P: Protocol> ServerMessage<P> {
    pub(crate) fn emit_send_logs(&self, channel_name: &str) {
        match self {
            ServerMessage::Message(message) => {
                let message_name = message.name();
                trace!(channel = ?channel_name, message = ?message_name, "Sending message");
                #[cfg(metrics)]
                metrics::increment_counter!("send_message", "channel" => channel_name, "message" => message_name);
            }
            ServerMessage::Replication(message) => {
                let _span = info_span!("send replication message", channel = ?channel_name, group_id = ?message.group_id);
                #[cfg(metrics)]
                metrics::increment_counter!("send_replication_actions");
                match &message.data {
                    ReplicationMessageData::Actions(m) => {
                        for (entity, actions) in &m.actions {
                            let _span = info_span!("send replication actions", ?entity);
                            if actions.spawn {
                                trace!("Send entity spawn");
                                #[cfg(metrics)]
                                metrics::increment_counter!("send_entity_spawn");
                            }
                            if actions.despawn {
                                trace!("Send entity despawn");
                                #[cfg(metrics)]
                                metrics::increment_counter!("send_entity_despawn");
                            }
                            if !actions.insert.is_empty() {
                                let components = actions
                                    .insert
                                    .iter()
                                    .map(|c| c.into())
                                    .collect::<Vec<P::ComponentKinds>>();
                                trace!(?components, "Sending component insert");
                                #[cfg(metrics)]
                                {
                                    for component in components {
                                        metrics::increment_counter!("send_component_insert", "component" => kind);
                                    }
                                }
                            }
                            if !actions.remove.is_empty() {
                                trace!(?actions.remove, "Sending component remove");
                                #[cfg(metrics)]
                                {
                                    for kind in actions.remove {
                                        metrics::increment_counter!("send_component_remove", "component" => kind);
                                    }
                                }
                            }
                            if !actions.updates.is_empty() {
                                let components = actions
                                    .updates
                                    .iter()
                                    .map(|c| c.into())
                                    .collect::<Vec<P::ComponentKinds>>();
                                trace!(?components, "Sending component update");
                                #[cfg(metrics)]
                                {
                                    for component in components {
                                        metrics::increment_counter!("send_component_update", "component" => kind);
                                    }
                                }
                            }
                        }
                    }
                    ReplicationMessageData::Updates(m) => {
                        for (entity, updates) in &m.updates {
                            let _span = info_span!("send replication updates", ?entity);
                            let components = updates
                                .iter()
                                .map(|c| c.into())
                                .collect::<Vec<P::ComponentKinds>>();
                            trace!(?components, "Sending component update");
                            #[cfg(metrics)]
                            {
                                for component in components {
                                    metrics::increment_counter!("send_component_update", "component" => kind);
                                }
                            }
                        }
                    }
                }
            }
            ServerMessage::Sync(message) => match message {
                SyncMessage::Ping(_) => {
                    trace!(channel = ?channel_name, "Sending ping");
                    #[cfg(metrics)]
                    metrics::increment_counter!("send_ping", "channel" => channel_name);
                }
                SyncMessage::Pong(_) => {
                    trace!(channel = ?channel_name, "Sending pong");
                    #[cfg(metrics)]
                    metrics::increment_counter!("send_pong", "channel" => channel_name);
                }
            },
        }
    }
}

// TODO: another option is to add ClientMessage and ServerMessage to ProtocolMessage
// then we can keep the shared logic in connection.mod. We just lose 1 bit everytime...
