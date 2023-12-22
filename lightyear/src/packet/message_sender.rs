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
pub struct MessageSender<M: BitSerializable> {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketBuilder,
    // TODO: add ordering of channels per priority
    pub(crate) channels: HashMap<ChannelKind, ChannelContainer>,
    pub(crate) channel_registry: ChannelRegistry,
    // TODO: can use Vec<ChannelKind, Vec<MessageId>> to be more efficient?
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    packet_to_message_ack_map: HashMap<PacketId, HashMap<ChannelKind, Vec<MessageAck>>>,
    writer: WriteWordBuffer,

    // MessageManager works because we only are only sending a single enum type
    _marker: PhantomData<M>,
}

impl<M: BitSerializable> MessageSender<M> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            packet_manager: PacketBuilder::new(),
            // TODO: we crate a channel receive that is unused
            channels: channel_registry.channels(),
            channel_registry: channel_registry.clone(),
            packet_to_message_ack_map: HashMap::new(),
            writer: WriteWordBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
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
            channel
                .sender
                .update(time_manager, ping_manager, tick_manager);
        }
    }

    /// Buffer a message to be sent on this connection
    /// Returns the message id associated with the message, if there is one
    pub fn buffer_send(
        &mut self,
        message: M,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<Option<MessageId>> {
        let channel = self
            .channels
            .get_mut(&channel_kind)
            .context("Channel not found")?;
        self.writer.start_write();
        message.encode(&mut self.writer)?;
        let message_bytes: Vec<u8> = self.writer.finish_write().into();
        Ok(channel.sender.buffer_send(message_bytes.into()))
    }

    /// Prepare buckets from the internal send buffers, and return the bytes to send
    // TODO: maybe pass TickManager instead of Tick? Find a more elegant way to pass extra data that might not be used?
    //  (ticks are not purely necessary without client prediction)
    //  maybe be generic over a Context ?
    pub fn send_packets(&mut self, current_tick: Tick) -> anyhow::Result<Vec<Payload>> {
        // Step 1. Get the list of packets to send from all channels
        // for each channel, prepare packets using the buffered messages that are ready to be sent
        // TODO: iterate through the channels in order of channel priority? (with accumulation)
        let mut data_to_send: BTreeMap<NetId, (VecDeque<SingleData>, VecDeque<FragmentData>)> =
            BTreeMap::new();
        for (channel_kind, channel) in self.channels.iter_mut() {
            let channel_id = self
                .channel_registry
                .get_net_from_kind(channel_kind)
                .context("cannot find channel id")?;
            channel.sender.collect_messages_to_send();
            if channel.sender.has_messages_to_send() {
                data_to_send.insert(*channel_id, channel.sender.send_packet());
            }
        }
        for (channel_id, (single_data, fragment_data)) in data_to_send.iter() {
            let channel_kind = self
                .channel_registry
                .get_kind_from_net_id(*channel_id)
                .unwrap();
            let channel_name = self.channel_registry.name(channel_kind).unwrap();
            trace!("sending data on channel {}", channel_name);
            // for single_data in single_data.iter() {
            //     info!(size = ?single_data.bytes.len(), "Single data");
            // }
            // for fragment_data in fragment_data.iter() {
            //     info!(size = ?fragment_data.bytes.len(),
            //           id = ?fragment_data.fragment_id,
            //           num_fragments = ?fragment_data.num_fragments,
            //           "Fragment data");
            // }
        }

        let packets = self.packet_manager.build_packets(data_to_send);

        let mut bytes = Vec::new();
        for mut packet in packets {
            trace!(num_messages = ?packet.data.num_messages(), "sending packet");
            let packet_id = packet.header().packet_id;

            // set the current tick
            packet.header.tick = current_tick;

            // Step 2. Get the packets to send over the network
            let payload = self.packet_manager.encode_packet(&packet)?;
            bytes.push(payload);
            // io.send(payload, &self.remote_addr)?;

            // TODO: update this to be cleaner
            // TODO: should we update this to include fragment info as well?
            // Step 3. Update the packet_to_message_id_map (only for channels that care about acks)
            packet
                .message_acks()
                .iter()
                .try_for_each(|(channel_id, message_ack)| {
                    let channel_kind = self
                        .channel_registry
                        .get_kind_from_net_id(*channel_id)
                        .context("cannot find channel kind")?;
                    let channel = self
                        .channels
                        .get(channel_kind)
                        .context("Channel not found")?;
                    if channel.setting.mode.is_watching_acks() {
                        self.packet_to_message_ack_map
                            .entry(packet_id)
                            .or_default()
                            .entry(*channel_kind)
                            .or_default()
                            .extend_from_slice(message_ack);
                    }
                    Ok::<(), anyhow::Error>(())
                })?;
        }

        Ok(bytes)
    }
}
