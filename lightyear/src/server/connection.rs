//! Wrapper around [`crate::connection::Connection`] that adds server-specific functionality
use bevy::utils::Duration;

use crate::_reexport::{EntityUpdatesChannel, InputMessage, PingChannel};
use anyhow::Result;
use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::World;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace, trace_span};

use crate::channel::senders::ChannelSend;
use crate::client::sync::SyncConfig;
use crate::connection::events::{ConnectionEvents, IterMessageEvent};
use crate::connection::message::{ClientMessage, ProtocolMessage, ServerMessage};
use crate::inputs::input_buffer::InputBuffer;
use crate::packet::message_manager::MessageManager;
use crate::packet::message_receivers::MessageReceiver;
use crate::packet::message_sender::MessageSender;
use crate::packet::packet_manager::Payload;
use crate::prelude::{ChannelKind, MapEntities, NetworkTarget};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::Protocol;
use crate::serialize::reader::ReadBuffer;
use crate::shared::ping::manager::{PingConfig, PingManager};
use crate::shared::ping::message::SyncMessage;
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
use crate::shared::replication::ReplicationMessage;
use crate::shared::replication::ReplicationMessageData;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// Wrapper around a [`crate::connection::Connection`] with client-specific logic
/// (handling player inputs, and syncing the time between client and server)
pub struct Connection<P: Protocol> {
    pub message_manager: MessageManager,
    pub(crate) replication_sender: ReplicationSender<P>,
    pub(crate) replication_receiver: ReplicationReceiver<P>,
    pub(crate) events: ConnectionEvents<P>,

    pub(crate) ping_manager: PingManager,
    /// Stores the inputs that we have received from the client.
    pub(crate) input_buffer: InputBuffer<P::Input>,
    /// Stores the last input we have received from the client.
    /// In case we are missing the client input for a tick, we will fallback to using this.
    pub(crate) last_input: Option<P::Input>,
    // TODO: maybe don't do any replication until connection is synced?
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry, ping_config: &PingConfig) -> Self {
        // create the message manager and the channels
        let mut message_manager = MessageManager::new(channel_registry);
        // get the acks-tracker for entity updates
        let update_acks_tracker = message_manager
            .channels
            .get_mut(&ChannelKind::of::<EntityUpdatesChannel>())
            .unwrap()
            .sender
            .subscribe_acks();
        let replication_sender = ReplicationSender::new(update_acks_tracker);
        let replication_receiver = ReplicationReceiver::new();
        Self {
            message_manager,
            replication_sender,
            replication_receiver,
            ping_manager: PingManager::new(ping_config),
            input_buffer: InputBuffer::default(),
            last_input: None,
            events: ConnectionEvents::default(),
        }
    }

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
        let message = ServerMessage::<P>::Message(message);
        message.emit_send_logs(&channel_name);
        self.message_manager.buffer_send(message, channel)?;
        Ok(())
    }

    pub fn buffer_replication_messages(&mut self, tick: Tick) -> Result<()> {
        self.replication_sender
            .finalize(tick)
            .into_iter()
            .try_for_each(|(channel, group_id, message_data)| {
                let should_track_ack = matches!(message_data, ReplicationMessageData::Updates(_));
                let channel_name = self
                    .message_manager
                    .channel_registry
                    .name(&channel)
                    .unwrap_or("unknown")
                    .to_string();
                let message = ClientMessage::<P>::Replication(ReplicationMessage {
                    group_id,
                    data: message_data,
                });
                message.emit_send_logs(&channel_name);
                let message_id = self
                    .message_manager
                    .buffer_send(message, channel)?
                    .expect("The EntityUpdatesChannel should always return a message_id");

                // keep track of the group associated with the message, so we can handle receiving an ACK for that message_id later
                if should_track_ack {
                    self.replication_sender
                        .updates_message_id_to_group_id
                        .insert(message_id, group_id);
                }
                Ok(())
            })
    }

    pub fn receive(
        &mut self,
        world: &mut World,
        time_manager: &TimeManager,
    ) -> ConnectionEvents<P> {
        let _span = trace_span!("receive").entered();
        for (channel_kind, messages) in self.message_manager.read_messages::<ClientMessage<P>>() {
            let channel_name = self
                .message_manager
                .channel_registry
                .name(&channel_kind)
                .unwrap_or("unknown");
            let _span_channel = trace_span!("channel", channel = channel_name).entered();

            if !messages.is_empty() {
                trace!(?channel_name, "Received messages");
                for (tick, message) in messages.into_iter() {
                    // TODO: we shouldn't map the entities here!
                    //  - we should: order the entities in a group by topological sort (use MapEntities to check dependencies between entities).
                    //  - apply map_entities when we're in the stage of applying to the world.
                    //    - because then we read the first entity in the group; spawn it, and the next component that refers to that entity can be mapped successfully!
                    // map entities from remote to local
                    // message.map_entities(&self.replication_manager.entity_map);

                    // other message-handling logic
                    match message {
                        ClientMessage::Message(mut message, target) => {
                            // map any entities inside the message
                            message.map_entities(Box::new(
                                &self.replication_receiver.remote_entity_map,
                            ));
                            // buffer the message
                            self.events.push_message(channel_kind, message);

                            // TODO: use target!
                        }
                        ClientMessage::Replication(replication) => {
                            // buffer the replication message
                            self.replication_receiver.recv_message(replication, tick);
                        }
                        ClientMessage::Sync(ref sync) => {
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
                for (group, replication_list) in self.replication_receiver.read_messages() {
                    trace!(?group, ?replication_list, "read replication messages");
                    replication_list.into_iter().for_each(|(_, replication)| {
                        // TODO: we could include the server tick when this replication_message was sent.
                        self.replication_receiver
                            .apply_world(world, replication, &mut self.events);
                    });
                }
            }
        }

        // inputs
        if self.events.has_messages::<InputMessage<P::Input>>() {
            trace!("update input buffer");
            // this has the added advantage that we remove the InputMessages so we don't read them later
            let input_messages: Vec<_> = self
                .events
                .into_iter_messages::<InputMessage<P::Input>>()
                .map(|(input_message, _)| input_message)
                .collect();
            for input_message in input_messages {
                // info!("Received input message: {:?}", input_message);
                self.input_buffer.update_from_message(input_message);
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
                let message = ProtocolMessage::<P>::Sync(SyncMessage::Ping(ping));
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
                    let message = ProtocolMessage::<P>::Sync(SyncMessage::Pong(pong));
                    let channel = ChannelKind::of::<PingChannel>();
                    self.message_manager.buffer_send(message, channel)?;
                    Ok::<(), anyhow::Error>(())
                })?;
        }
        self.message_manager
            .send_packets(tick_manager.current_tick())
    }

    pub fn recv_packet(
        &mut self,
        reader: &mut impl ReadBuffer,
        tick_manager: &TickManager,
        bevy_tick: BevyTick,
    ) -> Result<()> {
        self.replication_sender.recv_update_acks(bevy_tick);
        let tick = self.message_manager.recv_packet(reader)?;
        debug!("Received server packet with tick: {:?}", tick);
        Ok(())
    }
}
