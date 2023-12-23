use std::collections::{BTreeMap, HashMap, VecDeque};
use std::marker::PhantomData;

use anyhow::{anyhow, Context};
use tracing::trace;

use crate::channel::builder::ChannelContainer;
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageAck, MessageId, SingleData};
use crate::packet::packet::{Packet, PacketId};
use crate::packet::packet_manager::{PacketBuilder, Payload, PACKET_BUFFER_CAPACITY};
use crate::protocol::channel::{ChannelKind, ChannelRegistry};
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::ReadWordBuffer;
use crate::serialize::wordbuffer::writer::WriteWordBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
// TODO: put the M here or in the functions?
pub struct MessageReceiver<M: BitSerializable> {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketBuilder,
    // TODO: add ordering of channels per priority
    pub(crate) channels: HashMap<ChannelKind, ChannelContainer>,
    pub(crate) channel_registry: ChannelRegistry,
    // TODO: can use Vec<ChannelKind, Vec<MessageId>> to be more efficient?
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    packet_to_message_ack_map: HashMap<PacketId, HashMap<ChannelKind, Vec<MessageAck>>>,

    // MessageManager works because we only are only sending a single enum type
    _marker: PhantomData<M>,
}

impl<M: BitSerializable> MessageReceiver<M> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            packet_manager: PacketBuilder::new(),
            channels: channel_registry.channels(),
            channel_registry: channel_registry.clone(),
            packet_to_message_ack_map: HashMap::new(),
            _marker: Default::default(),
        }
    }

    /// Update book-keeping
    pub fn update(
        &mut self,
        time_manager: &TimeManager,
        ping_manager: &PingManager,
        tick_manager: &TickManager,
    ) {
        self.packet_manager.header_manager.update(time_manager);
        for channel in self.channels.values_mut() {
            channel.receiver.update(time_manager, tick_manager);
        }
    }

    /// Process packet received over the network as raw bytes
    /// Update the acks, and put the messages from the packets in internal buffers
    /// Returns the tick of the packet
    pub fn recv_packet(&mut self, reader: &mut impl ReadBuffer) -> anyhow::Result<Tick> {
        // Step 1. Parse the packet
        let packet: Packet = self.packet_manager.decode_packet(reader)?;
        let tick = packet.header().tick;
        trace!(?packet, "Received packet");

        // TODO: if it's fragmented, put it in a buffer? while we wait for all the parts to be ready?
        //  maybe the channel can handle the fragmentation?

        // TODO: an option is to have an async task that is on the receiving side of the
        //  cross-beam channel which tell which packets have been received

        // Step 2. Update the packet acks (which packets have we received, and which of our packets
        // have been acked)
        let acked_packets = self
            .packet_manager
            .header_manager
            .process_recv_packet_header(packet.header());

        // Step 3. Update the list of messages that have been acked
        for acked_packet in acked_packets {
            if let Some(message_map) = self.packet_to_message_ack_map.remove(&acked_packet) {
                for (channel_kind, message_acks) in message_map {
                    // TODO: do this on sender side! maybe send a message via channel to the senders?
                    let channel = self
                        .channels
                        .get_mut(&channel_kind)
                        .context("Channel not found")?;
                    for message_ack in message_acks {
                        channel.sender.notify_message_delivered(&message_ack);
                    }
                }
            }
        }

        // Step 4. Put the messages from the packet in the internal buffers for each channel
        for (channel_net_id, messages) in packet.data.contents() {
            let channel_kind = self
                .channel_registry
                .get_kind_from_net_id(channel_net_id)
                .context(format!(
                    "Could not recognize net_id {} as a channel",
                    channel_net_id
                ))?;
            let channel = self
                .channels
                .get_mut(channel_kind)
                .ok_or_else(|| anyhow!("Channel not found"))?;
            for mut message in messages {
                message.set_tick(tick);
                channel.receiver.buffer_recv(message)?;
            }
        }
        Ok(tick)
    }

    /// Read all the messages in the internal buffers that are ready to be processed
    // TODO: this is where naia converts the messages to events and pushes them to an event queue
    //  let be conservative and just return the messages right now. We could switch to an iterator
    pub fn read_messages(&mut self) -> HashMap<ChannelKind, Vec<(Tick, M)>> {
        let mut map = HashMap::new();
        for (channel_kind, channel) in self.channels.iter_mut() {
            let mut messages = vec![];
            while let Some(single_data) = channel.receiver.read_message() {
                let mut reader = ReadWordBuffer::start_read(single_data.bytes.as_ref());
                let message = M::decode(&mut reader).expect("Could not decode message");
                // TODO: why do we need finish read? to check for errors?
                // reader.finish_read()?;

                // SAFETY: when we receive the message, we set the tick of the message to the header tick
                // so every message has a tick
                messages.push((single_data.tick.unwrap(), message));
            }
            if !messages.is_empty() {
                map.insert(*channel_kind, messages);
            }
        }
        map
    }
}

// TODO: have a way to update the channels about the messages that have been acked
