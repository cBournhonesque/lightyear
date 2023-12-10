/*!  A connection is a wrapper that lets us send message and apply replication
*/

// only public for proc macro
pub mod events;

pub(crate) mod message;

use crate::_reexport::PingChannel;
use anyhow::Result;
use bevy::prelude::{Entity, World};
use bitcode::__private::Serialize;
use serde::Deserialize;
use tracing::{trace, trace_span};

use crate::connection::events::ConnectionEvents;
use crate::connection::message::ProtocolMessage;
use crate::packet::message_manager::MessageManager;
use crate::packet::packet_manager::Payload;
use crate::prelude::MapEntities;
use crate::protocol::channel::{ChannelKind, ChannelRegistry};
use crate::protocol::Protocol;
use crate::serialize::reader::ReadBuffer;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::SyncMessage;
use crate::shared::replication::manager::ReplicationManager;
use crate::shared::replication::ReplicationMessage;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;
use crate::utils::named::Named;

/// Wrapper to send/receive messages via channels to a remote address
pub struct Connection<P: Protocol> {
    pub ping_manager: PingManager,
    pub message_manager: MessageManager<ProtocolMessage<P>>,
    pub(crate) replication_manager: ReplicationManager<P>,
    pub events: ConnectionEvents<P>,
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        Self {
            ping_manager: PingManager::new(ping_config),
            message_manager: MessageManager::new(channel_registry),
            replication_manager: ReplicationManager::default(),
            events: ConnectionEvents::new(),
        }
    }
}

impl<P: Protocol> Connection<P> {
    pub fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.message_manager
            .update(time_manager, &self.ping_manager, tick_manager);
        self.ping_manager.update(time_manager);
    }

    pub fn buffer_message(&mut self, message: P::Message, channel: ChannelKind) -> Result<()> {
        // TODO: i know channel names never change so i should be able to get them as static
        // TODO: just have a channel registry enum as well?
        let channel_name = self
            .message_manager
            .channel_registry
            .name(&channel)
            .unwrap_or("unknown")
            .to_string();
        let message = ProtocolMessage::Message(message);
        message.emit_send_logs(&channel_name);
        self.message_manager.buffer_send(message, channel)
    }

    /// Buffer any replication messages
    pub fn buffer_replication_messages(&mut self, tick: Tick) -> Result<()> {
        self.replication_manager
            .finalize(tick)
            .into_iter()
            .try_for_each(|(channel, message)| {
                let channel_name = self
                    .message_manager
                    .channel_registry
                    .name(&channel)
                    .unwrap_or("unknown")
                    .to_string();
                message.emit_send_logs(&channel_name);
                self.message_manager.buffer_send(message, channel)
            })
    }

    // TODO: make world optional? or separate receiving into messages and applying into world?
    /// Read messages received from buffer (either messages or replication events) and push them to events
    pub fn receive(
        &mut self,
        world: &mut World,
        time_manager: &TimeManager,
    ) -> ConnectionEvents<P> {
        let _span = trace_span!("receive").entered();
        for (channel_kind, messages) in self.message_manager.read_messages() {
            let channel_name = self
                .message_manager
                .channel_registry
                .name(&channel_kind)
                .unwrap_or("unknown");
            let _span_channel = trace_span!("channel", channel = channel_name).entered();

            if !messages.is_empty() {
                trace!(?channel_name, "Received messages");
                for (tick, mut message) in messages.into_iter() {
                    // map entities from remote to local
                    message.map_entities(&self.replication_manager.entity_map);

                    // other message-handling logic
                    match message {
                        ProtocolMessage::Message(message) => {
                            // buffer the message
                            self.events.push_message(channel_kind, message);
                        }
                        ProtocolMessage::Replication(replication) => {
                            // buffer the replication message
                            self.replication_manager.recv_message(replication, tick);
                        }
                        ProtocolMessage::Sync(ref sync) => {
                            match sync {
                                SyncMessage::Ping(ping) => {
                                    // prepare a pong in response (but do not send yet, because we need
                                    // to set the correct send time)
                                    self.ping_manager.buffer_pending_pong(ping, time_manager);
                                }
                                SyncMessage::Pong(pong) => {
                                    // process the pong
                                    self.ping_manager.process_pong(pong, time_manager);
                                }
                            }
                        }
                    }
                }

                // Check if we have any replication messages we can apply to the World (and emit events)
                // TODO: maybe only run apply world if the client is time-synced!
                //  that would mean that for now, apply_world only runs on client, and not on server :)
                for (group, replication_list) in self.replication_manager.read_messages() {
                    replication_list.into_iter().for_each(|replication| {
                        self.replication_manager
                            .apply_world(world, replication, &mut self.events);
                    });
                }
            }
        }

        // TODO: do i really need this? I could just create events in this function directly?
        //  why do i need to make events a field of the connection?
        //  is it because of push_connection?
        std::mem::replace(&mut self.events, ConnectionEvents::new())
    }

    /// Send packets that are ready to be sent
    pub fn send_packets(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Result<Vec<Payload>> {
        // update the ping manager with the actual send time
        // TODO: issues here: we would like to send the ping/pong messages immediately, otherwise the recorded current time is incorrect
        //   - can give infinity priority to this channel?
        //   - can write directly to io otherwise?
        if time_manager.is_ready_to_send() {
            // maybe send pings
            // same thing, we want the correct send time for the ping
            // (and not have the delay between when we prepare the ping and when we send the packet)
            if let Some(ping) = self.ping_manager.maybe_prepare_ping(time_manager) {
                trace!("Sending ping {:?}", ping);
                let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
                let channel = ChannelKind::of::<PingChannel>();
                self.message_manager.buffer_send(message, channel)?;
            }

            // prepare the pong messages with the correct send time
            self.ping_manager
                .take_pending_pongs()
                .into_iter()
                .try_for_each(|mut pong| {
                    trace!("Sending pong {:?}", pong);
                    // update the send time of the pong
                    pong.pong_sent_time = time_manager.current_time();
                    let message = ProtocolMessage::Sync(SyncMessage::Pong(pong));
                    let channel = ChannelKind::of::<PingChannel>();
                    self.message_manager.buffer_send(message, channel)
                })?;
        }
        self.message_manager
            .send_packets(tick_manager.current_tick())
    }

    /// Receive a packet and buffer it
    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> Result<Tick> {
        self.message_manager.recv_packet(reader)
    }
}
