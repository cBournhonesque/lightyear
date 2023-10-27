use anyhow::Result;
use bevy::prelude::{Entity, World};
use bitcode::__private::Serialize;
use serde::Deserialize;
use tracing::{debug, trace_span};

use crate::connection::events::ConnectionEvents;
use crate::packet::message_manager::MessageManager;
use crate::replication::manager::ReplicationManager;
use crate::replication::ReplicationMessage;
use crate::{ChannelKind, ChannelRegistry, Protocol, ReadBuffer, WriteBuffer};

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct Connection<P: Protocol> {
    pub message_manager: MessageManager<ProtocolMessage<P>>,
    pub replication_manager: ReplicationManager<P>,
    pub events: ConnectionEvents<P>,
}

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Serialize, Deserialize, Clone)]
pub enum ProtocolMessage<P: Protocol> {
    Message(P::Message),
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
}

impl<P: Protocol> ProtocolMessage<P> {
    fn push_to_events(self, channel_kind: ChannelKind, events: &mut ConnectionEvents<P>) {
        match self {
            ProtocolMessage::Message(message) => {
                events.push_message(channel_kind, message);
            }
            ProtocolMessage::Replication(replication) => match replication {
                ReplicationMessage::SpawnEntity(entity, components) => {
                    events.push_spawn(entity);
                    for component in components {
                        events.push_insert_component(entity, component);
                    }
                }
                _ => {
                    // todo!()
                }
            },
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
    pub fn buffer_message(&mut self, message: P::Message, channel: ChannelKind) -> Result<()> {
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
    pub fn receive(&mut self, world: &mut World) -> ConnectionEvents<P> {
        trace_span!("receive").entered();
        for (channel_kind, messages) in self.message_manager.read_messages() {
            debug!(?channel_kind, "Received messages");
            for message in messages {
                // TODO: maybe we only need the component kind in the events, so we don't need to clone the message!
                // apply replication messages to the world
                self.replication_manager.apply_world(world, message.clone());
                // update events
                message.push_to_events(channel_kind, &mut self.events);
            }
        }

        // TODO: do i really need this? I could just create events in this function directly?
        //  why do i need to make events a field of the connection?
        std::mem::replace(&mut self.events, ConnectionEvents::new())
    }

    /// Send packets that are ready to be sent
    pub fn send_packets(&mut self) -> Result<Vec<impl WriteBuffer>> {
        self.message_manager.send_packets()
    }

    /// Receive a packet and buffer it
    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> Result<()> {
        self.message_manager.recv_packet(reader)
    }
}
