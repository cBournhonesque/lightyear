use anyhow::Result;
use bevy::prelude::{Entity, World};
use bitcode::encoding::Gamma;
use lightyear_derive::MessageInternal;
use serde::Deserialize;
use tracing::{trace, trace_span};

use crate::connection::events::ConnectionEvents;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet_manager::Payload;
use crate::replication::manager::ReplicationManager;
use crate::replication::ReplicationMessage;
use crate::tick::message::SyncMessage;
use crate::tick::Tick;
use crate::{
    ChannelKind, ChannelRegistry, Named, Protocol, ReadBuffer, TickManager, TimeManager,
    WriteBuffer,
};

// NOTE: we cannot have a message manager exclusively for messages, and a message manager for replication
// because prior to calling message_manager.recv() we don't know if the packet is a message or a replication event
// Also it would be inefficient because we would send separate packets for messages or replications, even though
// we can put them in the same packet

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct Connection<P: Protocol> {
    pub message_manager: MessageManager<ProtocolMessage<P>>,
    pub replication_manager: ReplicationManager<P>,
    pub events: ConnectionEvents<P>,
}

#[cfg_attr(feature = "debug", derive(Debug))]
// #[derive(MessageInternal, Serialize, Deserialize, Clone)]
#[derive(MessageInternal, Clone)]
#[serialize(nested)]
pub enum ProtocolMessage<P: Protocol> {
    Message(P::Message),
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
    // the reason why we include sync here instead of doing another MessageManager is so that
    // the sync messages can be added to packets that have other messages
    Sync(SyncMessage),
}

impl<P: Protocol> ProtocolMessage<P> {
    fn push_to_events(
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

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            message_manager: MessageManager::new(channel_registry),
            replication_manager: ReplicationManager::default(),
            events: ConnectionEvents::new(),
        }
    }
}

impl<P: Protocol> Connection<P> {
    pub fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.message_manager.update(time_manager, tick_manager);
    }

    pub fn buffer_message(&mut self, message: P::Message, channel: ChannelKind) -> Result<()> {
        #[cfg(feature = "metrics")]
        {
            // TODO: i know channel names never change so i should be able to get them as static
            // TODO: just have a channel registry enum as well?
            let channel_name = self
                .message_manager
                .channel_registry
                .name(&channel)
                .unwrap_or("unknown")
                .to_string();
            let message_name = message.name();
            // metrics::increment_counter!(format!("send_message.{}.{}", channel_name, message_name));
            metrics::increment_counter!("send_message", "channel" => channel_name, "message" => message_name);
        }
        // debug!("Buffering message to channel");
        let message = ProtocolMessage::Message(message);
        self.message_manager.buffer_send(message, channel)
    }

    pub fn buffer_spawn_entity(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        channel: ChannelKind,
    ) -> Result<()> {
        let message =
            ProtocolMessage::Replication(ReplicationMessage::SpawnEntity(entity, components));
        // TODO: add replication manager logic? (to check if the entity is already spawned or despawned, etc.)
        if self.replication_manager.send_entity_spawn(entity) {
            self.message_manager.buffer_send(message, channel)?
        }
        Ok(())
    }

    pub fn buffer_despawn_entity(&mut self, entity: Entity, channel: ChannelKind) -> Result<()> {
        let message = ProtocolMessage::Replication(ReplicationMessage::DespawnEntity(entity));
        self.message_manager.buffer_send(message, channel)
    }

    /// Buffer a component insert for an entity
    pub fn buffer_component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        channel: ChannelKind,
    ) -> Result<()> {
        self.replication_manager
            .send_component_insert(entity, component, channel);
        Ok(())
    }

    /// Buffer a component remove for an entity
    pub fn buffer_component_remove(
        &mut self,
        entity: Entity,
        component: P::ComponentKinds,
        channel: ChannelKind,
    ) -> Result<()> {
        // TODO: maybe don't send the component remove if the entity is despawning?
        let message =
            ProtocolMessage::Replication(ReplicationMessage::RemoveComponent(entity, component));
        self.message_manager.buffer_send(message, channel)
    }

    /// Buffer a component insert for an entity
    pub fn buffer_update_entity_single_component(
        &mut self,
        entity: Entity,
        component: P::Components,
        channel: ChannelKind,
    ) -> Result<()> {
        #[cfg(feature = "metrics")]
        {
            let channel_name = self
                .message_manager
                .channel_registry
                .name(&channel)
                .unwrap_or("unknown");
            let component_kind: P::ComponentKinds = (&component).into();
            metrics::increment_counter!(format!(
                "single_component_update.{}.{:?}",
                channel_name, component_kind
            ));
        }
        self.replication_manager
            .send_entity_update_single_component(entity, component, channel);
        Ok(())
    }

    /// Finalize any messages that we need to send for replication
    pub fn prepare_replication_send(&mut self) {
        self.replication_manager
            .prepare_send()
            .into_iter()
            .for_each(|(channel, message)| {
                self.message_manager.buffer_send(message, channel);
            })
    }

    pub fn buffer_update_entity(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        channel: ChannelKind,
    ) -> Result<()> {
        let message =
            ProtocolMessage::Replication(ReplicationMessage::EntityUpdate(entity, components));
        // TODO: add replication manager logic? (to check if the entity is already spawned or despawned, etc.)
        //  e.g. should we still send updates if the entity is despawning?
        self.message_manager.buffer_send(message, channel)
    }

    // TODO: make world optional? or separate receiving into messages and applying into world?
    /// Read messages received from buffer (either messages or replication events) and push them to events
    pub fn receive(
        &mut self,
        world: &mut World,
        time_manager: &TimeManager,
    ) -> ConnectionEvents<P> {
        trace_span!("receive").entered();
        for (channel_kind, messages) in self.message_manager.read_messages() {
            let channel_name = self
                .message_manager
                .channel_registry
                .name(&channel_kind)
                .unwrap_or("unknown");
            trace_span!("channel", channel = channel_name).entered();

            if !messages.is_empty() {
                trace!(?channel_name, "Received messages");
                for message in messages {
                    // TODO: maybe only run apply world if the client is time-synced!
                    //  that would mean that for now, apply_world only runs on client, and not on server :)

                    // TODO: maybe we only need the component kind in the events, so we don't need to clone the message!
                    // apply replication messages to the world
                    self.replication_manager.apply_world(world, message.clone());
                    // update events
                    message.push_to_events(channel_kind, &mut self.events, time_manager);
                }
            }
        }

        // HERE: we are clearing the input buffers from every connection, which is not what we want!

        // TODO: do i really need this? I could just create events in this function directly?
        //  why do i need to make events a field of the connection?
        //  is it because of push_connection?
        std::mem::replace(&mut self.events, ConnectionEvents::new())
    }

    /// Send packets that are ready to be sent
    pub fn send_packets(&mut self, tick_manager: &TickManager) -> Result<Vec<Payload>> {
        // if !self.is_ready_to_send() {
        //     info!("Not ready to send packets");
        //     return Ok(vec![]);
        // }
        self.message_manager
            .send_packets(tick_manager.current_tick())
    }

    /// Receive a packet and buffer it
    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> Result<Tick> {
        self.message_manager.recv_packet(reader)
    }
}
