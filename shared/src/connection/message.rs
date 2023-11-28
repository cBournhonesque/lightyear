use crate::connection::events::ConnectionEvents;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::shared::replication::ReplicationMessage;
use crate::tick::message::SyncMessage;
use crate::tick::time::TimeManager;
use crate::utils::named::Named;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Serialize, Deserialize, Clone)]
pub enum ProtocolMessage<P: Protocol> {
    Message(P::Message),
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    Sync(SyncMessage),
}

impl<P: Protocol> ProtocolMessage<P> {
    pub(crate) fn push_to_events(
        self,
        channel_kind: ChannelKind,
        events: &mut ConnectionEvents<P>,
        time_manager: &TimeManager,
    ) {
        match self {
            ProtocolMessage::Message(message) => {
                #[cfg(feature = "metrics")]
                {
                    let message_name = message.name();
                    metrics::increment_counter!(format!("receive_message.{}", message_name));
                }
                events.push_message(channel_kind, message);
            }
            ProtocolMessage::Replication(replication) => match replication {
                ReplicationMessage::SpawnEntity(entity, components) => {
                    events.push_spawn(entity);
                    for component in components {
                        events.push_insert_component(entity, (&component).into());
                    }
                }
                ReplicationMessage::DespawnEntity(entity) => {
                    events.push_despawn(entity);
                }
                ReplicationMessage::InsertComponent(entity, component) => {
                    events.push_insert_component(entity, (&component).into());
                }
                ReplicationMessage::RemoveComponent(entity, component_kind) => {
                    events.push_remove_component(entity, component_kind);
                }
                ReplicationMessage::EntityUpdate(entity, components) => {
                    for component in components {
                        events.push_update_component(entity, (&component).into());
                    }
                }
            },
            ProtocolMessage::Sync(mut sync) => {
                match sync {
                    SyncMessage::TimeSyncPing(ref mut ping) => {
                        // set the time received
                        ping.ping_received_time = Some(time_manager.current_time());
                    }
                    _ => {}
                };
                events.push_sync(sync);
            }
        }
    }
}
