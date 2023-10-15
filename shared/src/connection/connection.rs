use anyhow::Result;
use bevy_ecs::prelude::{Entity, World};
use bitcode::__private::Serialize;
use serde::Deserialize;

use crate::connection::events::Events;
use crate::packet::message_manager::MessageManager;
use crate::replication::manager::ReplicationManager;
use crate::replication::ReplicationMessage;
use crate::{Channel, ChannelKind, ChannelRegistry, Protocol, ReadBuffer, WriteBuffer};

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct Connection<P: Protocol> {
    pub message_manager: MessageManager<ProtocolMessage<P>>,
    pub replication_manager: ReplicationManager,
    pub events: Events<P>,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum ProtocolMessage<P: Protocol> {
    Message(P::Message),
    Replication(ReplicationMessage<P::Components, P::ComponentKinds>),
}

impl<P: Protocol> ProtocolMessage<P> {
    fn push_to_events(self, channel_kind: ChannelKind, events: &mut Events<P>) {
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
                    todo!()
                }
            },
        }
    }
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            message_manager: MessageManager::new(channel_registry),
            replication_manager: ReplicationManager::new(),
            events: Events::new(),
        }
    }
}

impl<P: Protocol> Connection<P> {
    pub fn buffer_message<C: Channel>(&mut self, message: P::Message) -> Result<()> {
        let message = ProtocolMessage::Message(message);
        self.message_manager.buffer_send::<C>(message)
    }

    pub fn buffer_spawn_entity<C: Channel>(&mut self, entity: Entity) -> Result<()> {
        let message = ProtocolMessage::Replication(ReplicationMessage::SpawnEntity(entity));
        // TODO: add replication manager logic? (to check if the entity is already spawned or despawned, etc.)
        self.message_manager.buffer_send::<C>(message)
    }

    pub fn buffer_despawn_entity<C: Channel>(&mut self, entity: Entity) -> Result<()> {
        let message = ProtocolMessage::Replication(ReplicationMessage::DespawnEntity(entity));
        self.message_manager.buffer_send::<C>(message)
    }

    // TODO: make world optional?
    /// Read messages received from buffer (either messages or replication events) and push them to events
    pub fn receive(&mut self, world: &mut World) -> Events<P> {
        for (channel_kind, messages) in self.message_manager.read_messages() {
            for message in messages {
                // TODO: maybe we only need the component kind in the events, so we don't need to copy the message!
                // update events
                message
                    .inner()
                    .push_to_events(channel_kind, &mut self.events);

                // apply replication messages to the world
                self.replication_manager.apply_world(world, message)
            }
        }

        // TODO: do i really need this? I could just create events in this function directly?
        //  why do i need to make events a field of the connection?
        std::mem::replace(&mut self.events, Events::new())
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
