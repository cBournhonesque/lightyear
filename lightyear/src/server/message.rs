use anyhow::Context;
use tracing::{info_span, trace};

use bitcode::__private::Fixed;
use bitcode::{Decode, Encode};

use crate::_reexport::{BitSerializable, MessageProtocol, ReadBuffer, WriteBuffer};
use crate::prelude::Protocol;
use crate::shared::ping::message::SyncMessage;
use crate::shared::replication::{ReplicationMessage, ReplicationMessageData};

#[derive(Encode, Decode, Clone, Debug)]
pub enum ServerMessage<P: Protocol> {
    #[bitcode_hint(frequency = 2)]
    #[bitcode(with_serde)]
    Message(P::Message),
    #[bitcode_hint(frequency = 3)]
    #[bitcode(with_serde)]
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    #[bitcode_hint(frequency = 1)]
    Sync(SyncMessage),
}

impl<P: Protocol> BitSerializable for ServerMessage<P> {
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        writer.encode(self, Fixed).context("could not encode")
        // writer.serialize(self)
    }
    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        reader.decode::<Self>(Fixed).context("could not decode")
        // reader.deserialize::<Self>()
    }
}

impl<P: Protocol> ServerMessage<P> {
    pub(crate) fn emit_send_logs(&self, channel_name: &str) {
        match self {
            ServerMessage::Message(message) => {
                let message_name = message.name();
                trace!(channel = ?channel_name, message = ?message_name, kind = ?message.kind(), "Sending message");
                #[cfg(metrics)]
                metrics::counter!("send_message", "channel" => channel_name, "message" => message_name).increment(1);
            }
            ServerMessage::Replication(message) => {
                let _span = info_span!("send replication message", channel = ?channel_name, group_id = ?message.group_id);
                #[cfg(metrics)]
                metrics::counter!("send_replication_actions").increment(1);
                match &message.data {
                    ReplicationMessageData::Actions(m) => {
                        for (entity, actions) in &m.actions {
                            let _span = info_span!("send replication actions", ?entity);
                            if actions.spawn {
                                trace!("Send entity spawn");
                                #[cfg(metrics)]
                                metrics::counter!("send_entity_spawn").increment(1);
                            }
                            if actions.despawn {
                                trace!("Send entity despawn");
                                #[cfg(metrics)]
                                metrics::counter!("send_entity_despawn").increment(1);
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
                                        metrics::counter!("send_component_insert", "component" => kind).increment(1);
                                    }
                                }
                            }
                            if !actions.remove.is_empty() {
                                trace!(?actions.remove, "Sending component remove");
                                #[cfg(metrics)]
                                {
                                    for kind in actions.remove {
                                        metrics::counter!("send_component_remove", "component" => kind).increment(1);
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
                                        metrics::counter!("send_component_update", "component" => kind).increment(1);
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
                                    metrics::counter!("send_component_update", "component" => kind)
                                        .increment(1);
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
                    metrics::counter!("send_ping", "channel" => channel_name).increment(1);
                }
                SyncMessage::Pong(_) => {
                    trace!(channel = ?channel_name, "Sending pong");
                    #[cfg(metrics)]
                    metrics::counter!("send_pong", "channel" => channel_name).increment(1);
                }
            },
        }
    }
}

// TODO: another option is to add ClientMessage and ServerMessage to ProtocolMessage
// then we can keep the shared logic in connection.mod. We just lose 1 bit everytime...
