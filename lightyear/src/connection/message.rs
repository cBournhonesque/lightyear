/*!
Provides a [`ProtocolMessage`] enum that is a wrapper around all the possible messages that can be sent over the network
*/
use crate::_reexport::{InputMessage, ShouldBeInterpolated, ShouldBePredicted, TickManager};
use crate::connection::events::ConnectionEvents;
use crate::prelude::{EntityMap, MapEntities};
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::shared::ping::message::SyncMessage;
use crate::shared::replication::ReplicationMessage;
use crate::shared::time_manager::TimeManager;
use crate::utils::named::Named;
use serde::{Deserialize, Serialize};
use tracing::{info, trace, trace_span};

// pub enum InternalMessage<P: Protocol> {
//     InputMessage(InputMessage<P::Input>),
// }
//
// pub enum InternalReplication<P: Protocol> {
//     ShouldBePredicted(ShouldBePredicted),
//     ShouldBeInterpolated(ShouldBeInterpolated),
// }

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Serialize, Deserialize, Clone)]
pub enum ProtocolMessage<P: Protocol> {
    Message(P::Message),
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    Sync(SyncMessage),
}

impl<P: Protocol> MapEntities for ProtocolMessage<P> {
    fn map_entities(&mut self, entity_map: &EntityMap) {
        match self {
            ProtocolMessage::Message(x) => {
                x.map_entities(entity_map);
            }
            ProtocolMessage::Replication(x) => {
                x.map_entities(entity_map);
            }
            ProtocolMessage::Sync(x) => {
                x.map_entities(entity_map);
            }
        }
    }
}

impl<P: Protocol> ProtocolMessage<P> {
    pub(crate) fn emit_send_logs(&self, channel_name: &str) {
        match self {
            ProtocolMessage::Message(message) => {
                let message_name = message.name();
                info!(channel = ?channel_name, message = ?message_name, "Sending message");
                #[cfg(metrics)]
                metrics::increment_counter!("send_message", "channel" => channel_name, "message" => message_name);
            }
            ProtocolMessage::Replication(message) => match message {
                ReplicationMessage::SpawnEntity(entity, components) => {
                    let components = components
                        .iter()
                        .map(|c| c.into())
                        .collect::<Vec<P::ComponentKinds>>();
                    info!(channel = ?channel_name, ?entity, ?components, "Sending entity spawn");
                    #[cfg(metrics)]
                    {
                        metrics::increment_counter!("send_entity_spawn", "channel" => channel_name);
                        for component in components {
                            let kind: P::ComponentKinds = component.into();
                            metrics::increment_counter!("send_component_insert", "channel" => channel_name, "component" => kind);
                        }
                    }
                }
                ReplicationMessage::DespawnEntity(entity) => {
                    info!(channel = ?channel_name, ?entity, "Sending entity despawn");
                    #[cfg(metrics)]
                    metrics::increment_counter!("send_entity_despawn", "channel" => channel_name);
                }
                ReplicationMessage::InsertComponent(entity, components) => {
                    let components = components
                        .iter()
                        .map(|c| c.into())
                        .collect::<Vec<P::ComponentKinds>>();
                    info!(channel = ?channel_name, ?entity, ?components, "Sending component insert");
                    #[cfg(metrics)]
                    {
                        for component in components {
                            let kind: P::ComponentKinds = component.into();
                            metrics::increment_counter!("send_component_insert", "channel" => channel_name, "component" => kind);
                        }
                    }
                }
                ReplicationMessage::RemoveComponent(entity, components) => {
                    info!(channel = ?channel_name, ?entity, ?components, "Sending component remove");
                    #[cfg(metrics)]
                    {
                        for component in components {
                            let kind: P::ComponentKinds = component.into();
                            metrics::increment_counter!("send_component_remove", "channel" => channel_name, "component" => kind);
                        }
                    }
                }
                ReplicationMessage::EntityUpdate(entity, components) => {
                    let components = components
                        .iter()
                        .map(|c| c.into())
                        .collect::<Vec<P::ComponentKinds>>();
                    info!(channel = ?channel_name, ?entity, ?components, "Sending entity update");
                    #[cfg(metrics)]
                    {
                        for component in components {
                            let kind: P::ComponentKinds = component.into();
                            metrics::increment_counter!("send_component_update", "channel" => channel_name, "component" => kind);
                        }
                    }
                }
            },
            ProtocolMessage::Sync(message) => match message {
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
    pub(crate) fn push_to_events(
        self,
        channel_kind: ChannelKind,
        events: &mut ConnectionEvents<P>,
        entity_map: &EntityMap,
        time_manager: &TimeManager,
    ) {
        let _span = trace_span!("receive");
        match self {
            ProtocolMessage::Message(message) => {
                events.push_message(channel_kind, message);
            }
            ProtocolMessage::Replication(replication) => match replication {
                ReplicationMessage::SpawnEntity(entity, components) => {
                    // convert the remote entity to the local before sending to events
                    // if we can't find the local entity, just use the remote
                    let local_entity = *entity_map.get_local(entity).unwrap_or(&entity);
                    events.push_spawn(local_entity);
                    for component in components {
                        events.push_insert_component(local_entity, (&component).into());
                    }
                }
                ReplicationMessage::DespawnEntity(entity) => {
                    let local_entity = *entity_map.get_local(entity).unwrap_or(&entity);
                    events.push_despawn(local_entity);
                }
                ReplicationMessage::InsertComponent(entity, components) => {
                    let local_entity = *entity_map.get_local(entity).unwrap_or(&entity);
                    for component in components {
                        events.push_insert_component(local_entity, (&component).into());
                    }
                }
                ReplicationMessage::RemoveComponent(entity, component_kinds) => {
                    let local_entity = *entity_map.get_local(entity).unwrap_or(&entity);
                    for component_kind in component_kinds {
                        events.push_remove_component(local_entity, component_kind);
                    }
                }
                ReplicationMessage::EntityUpdate(entity, components) => {
                    let local_entity = *entity_map.get_local(entity).unwrap_or(&entity);
                    for component in components {
                        events.push_update_component(local_entity, (&component).into());
                    }
                }
            },
            _ => {}
        }
    }
}
