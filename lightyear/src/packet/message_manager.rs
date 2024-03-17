use std::collections::{HashMap, VecDeque};

use anyhow::{anyhow, Context};
use bevy::ptr::UnsafeCellDeref;
use bevy::reflect::Reflect;
use bitcode::buffer::BufferTrait;
use bitcode::word_buffer::WordBuffer;
use crossbeam_channel::Receiver;
use tracing::{info, trace};

use crate::channel::builder::ChannelContainer;
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageAck, MessageId, SingleData};
use crate::packet::packet::{Packet, PacketId, MTU_PAYLOAD_BYTES};
use crate::packet::packet_manager::{PacketBuilder, Payload, PACKET_BUFFER_CAPACITY};
use crate::packet::priority_manager::{PriorityConfig, PriorityManager};
use crate::protocol::channel::{ChannelKind, ChannelRegistry};
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::{BufferPool, ReadWordBuffer};
use crate::serialize::wordbuffer::writer::WriteWordBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

// TODO: hard to split message manager into send/receive because the acks need both the send side and receive side
//  maybe have a separate actor for acks?

pub const DEFAULT_MESSAGE_PRIORITY: f32 = 1.0;

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct MessageManager {
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketBuilder,
    priority_manager: PriorityManager,
    pub(crate) channels: HashMap<ChannelKind, ChannelContainer>,
    pub(crate) channel_registry: ChannelRegistry,
    // TODO: can use Vec<ChannelKind, Vec<MessageId>> to be more efficient?
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    packet_to_message_ack_map: HashMap<PacketId, HashMap<ChannelKind, Vec<MessageAck>>>,
    writer: WriteWordBuffer,
    // read_buffer: WordBuffer,
    reader_pool: BufferPool,
}

impl MessageManager {
    pub fn new(channel_registry: &ChannelRegistry, priority_config: PriorityConfig) -> Self {
        Self {
            packet_manager: PacketBuilder::new(),
            priority_manager: PriorityManager::new(priority_config),
            channels: channel_registry.channels(),
            channel_registry: channel_registry.clone(),
            packet_to_message_ack_map: HashMap::new(),
            writer: WriteWordBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
            // TODO: it looks like we don't really need the pool this case, we can just keep re-using the same buffer
            reader_pool: BufferPool::new(1),
            // read_buffer: WordBuffer::with_capacity(MTU_PAYLOAD_BYTES),
        }
    }

    pub(crate) fn get_replication_update_send_receiver(&mut self) -> Receiver<MessageId> {
        self.priority_manager
            .subscribe_replication_update_sent_messages()
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
            channel.receiver.update(time_manager, tick_manager);
        }
    }

    /// Buffer a message to be sent on this connection
    /// Returns the message id associated with the message, if there is one
    pub fn buffer_send<M: BitSerializable>(
        &mut self,
        message: M,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<Option<MessageId>> {
        self.buffer_send_with_priority(message, channel_kind, DEFAULT_MESSAGE_PRIORITY)
    }

    // TODO: for priority sending, we might want to include the tick at which we buffered the message
    //  because the tick at which the message is sent is not guaranteed to be the same as the tick at which
    //  it was buffered. (which normally is the case for replication messages)
    //  This is also not the case in general for Messages. You could buffer a message at PreUpdate on tick 7, but
    //  then it actually gets sent on tick 8 so the packet header says tick 8. In some cases it could be important to
    //  know the exact tick at which the message was buffered.
    //  For input messages it's not a problem because we include the tick directly in the message.
    //  For now let's keep it simple and simply add the tick when we try to send the message (so this works for all
    //  replication messages), not when we buffer it. For cases where it's necessary to know the tick when the message
    //  was buffered, the user can just include the tick in the message itself.
    /// Buffer a message to be sent on this connection
    /// Returns the message id associated with the message, if there is one
    pub fn buffer_send_with_priority<M: BitSerializable>(
        &mut self,
        message: M,
        channel_kind: ChannelKind,
        priority: f32,
    ) -> anyhow::Result<Option<MessageId>> {
        let channel = self
            .channels
            .get_mut(&channel_kind)
            .context("Channel not found")?;
        self.writer.start_write();
        message.encode(&mut self.writer)?;
        let message_bytes: Vec<u8> = self.writer.finish_write().into();
        Ok(channel.sender.buffer_send(message_bytes.into(), priority))
    }

    /// Prepare buckets from the internal send buffers, and return the bytes to send
    // TODO: maybe pass TickManager instead of Tick? Find a more elegant way to pass extra data that might not be used?
    //  (ticks are not purely necessary without client prediction)
    //  maybe be generic over a Context ?
    pub fn send_packets(&mut self, current_tick: Tick) -> anyhow::Result<Vec<Payload>> {
        // Step 1. Get the list of packets to send from all channels
        // for each channel, prepare packets using the buffered messages that are ready to be sent
        // TODO: iterate through the channels in order of channel priority? (with accumulation)
        let mut data_to_send: Vec<(NetId, (VecDeque<SingleData>, VecDeque<FragmentData>))> = vec![];
        let mut has_data_to_send = false;
        for (channel_kind, channel) in self.channels.iter_mut() {
            let channel_id = self
                .channel_registry
                .get_net_from_kind(channel_kind)
                .context("cannot find channel id")?;
            channel.sender.collect_messages_to_send();
            if channel.sender.has_messages_to_send() {
                let (single_data, fragment_data) = channel.sender.send_packet();
                if !single_data.is_empty() || !fragment_data.is_empty() {
                    has_data_to_send = true;
                }
                data_to_send.push((*channel_id, (single_data, fragment_data)));
            }
        }
        // return early if there are no messages to send
        if !has_data_to_send {
            return Ok(vec![]);
        }

        // priority manager: get the list of messages we can send according to the rate limiter
        //  (the other messages are stored in an internal buffer)
        let (data_to_send, num_bytes_added_to_limiter) = self.priority_manager.priority_filter(
            data_to_send,
            &self.channel_registry,
            current_tick,
        );

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

        // adjust the real amount of bytes that we sent through the limiter (to account for the actual packet size)
        if self.priority_manager.config.enabled {
            let total_bytes_sent = bytes.iter().map(|b| b.len() as u32).sum::<u32>();
            if let Ok(remaining_bytes_to_add) =
                (total_bytes_sent - num_bytes_added_to_limiter).try_into()
            {
                let _ = self
                    .priority_manager
                    .limiter
                    .check_n(remaining_bytes_to_add);
            }
        }

        Ok(bytes)
    }

    /// Process packet received over the network as raw bytes
    /// Update the acks, and put the messages from the packets in internal buffers
    /// Returns the tick of the packet
    pub fn recv_packet(&mut self, packet: Packet) -> anyhow::Result<Tick> {
        // Step 1. Parse the packet
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
            trace!(
                "received {:?} messages from channel: {:?}",
                messages,
                channel_kind
            );
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
    pub fn read_messages<M: BitSerializable>(&mut self) -> HashMap<ChannelKind, Vec<(Tick, M)>> {
        let mut map = HashMap::new();
        for (channel_kind, channel) in self.channels.iter_mut() {
            let mut messages = vec![];
            while let Some(single_data) = channel.receiver.read_message() {
                trace!(?channel_kind, "reading message: {:?}", single_data);
                // TODO: in this case, it looks like we might not need the pool?
                //  we can just have a single buffer, and keep re-using that buffer
                trace!(pool_len = ?self.reader_pool.0.len(), "read from message manager");
                let mut reader = self.reader_pool.start_read(single_data.bytes.as_ref());
                let message = M::decode(&mut reader).expect("Could not decode message");
                // return the buffer to the pool
                self.reader_pool.attach(reader);

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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bevy::utils::Duration;

    use crate::_reexport::*;
    use crate::packet::message::MessageId;
    use crate::packet::packet::FRAGMENT_SIZE;
    use crate::packet::priority_manager::PriorityConfig;
    use crate::prelude::*;
    use crate::tests::protocol::*;

    use super::*;

    #[test]
    /// We want to test that we can send/receive messages over a connection
    fn test_message_manager_single_message() -> Result<(), anyhow::Error> {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::TRACE)
        //     .init();

        let time_manager = TimeManager::new(Duration::default());
        let protocol = protocol();

        // Create message managers
        let mut client_message_manager =
            MessageManager::new(protocol.channel_registry(), PriorityConfig::default());
        let mut server_message_manager =
            MessageManager::new(protocol.channel_registry(), PriorityConfig::default());

        // client: buffer send messages, and then send
        let message = MyMessageProtocol::Message1(Message1("1".to_string()));
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        client_message_manager.buffer_send(message.clone(), channel_kind_1)?;
        client_message_manager.buffer_send(message.clone(), channel_kind_2)?;
        let mut packet_bytes = client_message_manager.send_packets(Tick(0))?;
        assert_eq!(
            client_message_manager.packet_to_message_ack_map,
            HashMap::from([(
                PacketId(0),
                HashMap::from([(
                    channel_kind_2,
                    vec![MessageAck {
                        message_id: MessageId(0),
                        fragment_id: None,
                    }]
                )])
            )])
        );

        // server: receive bytes from the sent messages, then process them into messages
        for packet_byte in packet_bytes.iter_mut() {
            let packet = Packet::decode(&mut ReadWordBuffer::start_read(packet_byte.as_slice()))?;
            server_message_manager.recv_packet(packet)?;
        }
        let mut data = server_message_manager.read_messages();
        assert_eq!(
            data.get(&channel_kind_1).unwrap(),
            &vec![(Tick(0), message.clone())]
        );
        assert_eq!(
            data.get(&channel_kind_2).unwrap(),
            &vec![(Tick(0), message.clone())]
        );

        // Confirm what happens if we try to receive but there is nothing on the io
        data = server_message_manager.read_messages();
        assert!(data.is_empty());

        // Check the state of the packet headers
        assert_eq!(
            client_message_manager
                .packet_manager
                .header_manager
                .next_packet_id(),
            PacketId(1)
        );
        assert!(client_message_manager
            .packet_manager
            .header_manager
            .sent_packets_not_acked()
            .contains_key(&PacketId(0)));

        // Server sends back a message
        server_message_manager.buffer_send(message.clone(), channel_kind_1)?;
        let mut packet_bytes = server_message_manager.send_packets(Tick(0))?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for packet_byte in packet_bytes.iter_mut() {
            let packet = Packet::decode(&mut ReadWordBuffer::start_read(packet_byte.as_slice()))?;
            client_message_manager.recv_packet(packet)?;
        }

        // Check that reliability works correctly
        assert_eq!(client_message_manager.packet_to_message_ack_map.len(), 0);
        // TODO: check that client_channel_1's sender's unacked messages is empty
        // let client_channel_1 = client_connection.channels.get(&channel_kind_1).unwrap();
        // assert_eq!(client_channel_1.sender.)
        Ok(())
    }

    #[test]
    /// We want to test that we can send/receive messages over a connection
    fn test_message_manager_fragment_message() -> Result<(), anyhow::Error> {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::TRACE)
        //     .init();
        let protocol = protocol();

        // Create message managers
        let mut client_message_manager =
            MessageManager::new(protocol.channel_registry(), PriorityConfig::default());
        let mut server_message_manager =
            MessageManager::new(protocol.channel_registry(), PriorityConfig::default());

        // client: buffer send messages, and then send
        const MESSAGE_SIZE: usize = (1.5 * FRAGMENT_SIZE as f32) as usize;

        let data = std::str::from_utf8(&[0; MESSAGE_SIZE]).unwrap().to_string();
        let message = MyMessageProtocol::Message1(Message1(data));
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        client_message_manager.buffer_send(message.clone(), channel_kind_1)?;
        client_message_manager.buffer_send(message.clone(), channel_kind_2)?;
        let mut packet_bytes = client_message_manager.send_packets(Tick(0))?;
        assert_eq!(packet_bytes.len(), 4);
        assert_eq!(
            client_message_manager.packet_to_message_ack_map,
            HashMap::from([
                (
                    PacketId(2),
                    HashMap::from([(
                        channel_kind_2,
                        vec![MessageAck {
                            message_id: MessageId(0),
                            fragment_id: Some(0),
                        },]
                    )])
                ),
                (
                    PacketId(3),
                    HashMap::from([(
                        channel_kind_2,
                        vec![MessageAck {
                            message_id: MessageId(0),
                            fragment_id: Some(1),
                        }]
                    )])
                ),
            ])
        );

        // server: receive bytes from the sent messages, then process them into messages
        for packet_byte in packet_bytes.iter_mut() {
            let packet = Packet::decode(&mut ReadWordBuffer::start_read(packet_byte.as_slice()))?;
            server_message_manager.recv_packet(packet)?;
        }
        let mut data = server_message_manager.read_messages();
        assert_eq!(
            data.get(&channel_kind_1).unwrap(),
            &vec![(Tick(0), message.clone())]
        );
        assert_eq!(
            data.get(&channel_kind_2).unwrap(),
            &vec![(Tick(0), message.clone())]
        );

        // Confirm what happens if we try to receive but there is nothing on the io
        data = server_message_manager.read_messages();
        assert!(data.is_empty());

        // Check the state of the packet headers
        assert_eq!(
            client_message_manager
                .packet_manager
                .header_manager
                .next_packet_id(),
            PacketId(4)
        );
        assert!(client_message_manager
            .packet_manager
            .header_manager
            .sent_packets_not_acked()
            .contains_key(&PacketId(0)));
        assert!(client_message_manager
            .packet_manager
            .header_manager
            .sent_packets_not_acked()
            .contains_key(&PacketId(1)));

        // Server sends back a message
        server_message_manager.buffer_send(
            MyMessageProtocol::Message1(Message1("b".to_string())),
            channel_kind_1,
        )?;
        let mut packet_bytes = server_message_manager.send_packets(Tick(0))?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for packet_byte in packet_bytes.iter_mut() {
            let packet = Packet::decode(&mut ReadWordBuffer::start_read(packet_byte.as_slice()))?;
            client_message_manager.recv_packet(packet)?;
        }

        // Check that reliability works correctly
        assert_eq!(client_message_manager.packet_to_message_ack_map.len(), 0);
        // TODO: check that client_channel_1's sender's unacked messages is empty
        // let client_channel_1 = client_connection.channels.get(&channel_kind_1).unwrap();
        // assert_eq!(client_channel_1.sender.)
        Ok(())
    }

    #[test]
    fn test_notify_ack() -> anyhow::Result<()> {
        let protocol = protocol();

        // Create message managers
        let mut client_message_manager =
            MessageManager::new(protocol.channel_registry(), PriorityConfig::default());
        let mut server_message_manager =
            MessageManager::new(protocol.channel_registry(), PriorityConfig::default());

        let update_acks_tracker = client_message_manager
            .channels
            .get_mut(&ChannelKind::of::<Channel2>())
            .unwrap()
            .sender
            .subscribe_acks();

        let message_id = client_message_manager
            .buffer_send(MyMessageProtocol::Message2(Message2(1)), Channel2::kind())?
            .unwrap();
        assert_eq!(message_id, MessageId(0));
        let mut payloads = client_message_manager.send_packets(Tick(0))?;
        assert_eq!(
            client_message_manager.packet_to_message_ack_map,
            HashMap::from([(
                PacketId(0),
                HashMap::from([(
                    Channel2::kind(),
                    vec![MessageAck {
                        message_id,
                        fragment_id: None,
                    }]
                )])
            )])
        );

        // server: receive bytes from the sent messages, then process them into messages
        for packet_byte in payloads.iter_mut() {
            let packet = Packet::decode(&mut ReadWordBuffer::start_read(packet_byte.as_slice()))?;
            server_message_manager.recv_packet(packet)?;
        }

        // Server sends back a message (to ack the message)
        server_message_manager
            .buffer_send(MyMessageProtocol::Message2(Message2(1)), Channel2::kind())?;
        let mut packet_bytes = server_message_manager.send_packets(Tick(0))?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for packet_byte in packet_bytes.iter_mut() {
            let packet = Packet::decode(&mut ReadWordBuffer::start_read(packet_byte.as_slice()))?;
            client_message_manager.recv_packet(packet)?;
        }

        assert_eq!(update_acks_tracker.try_recv()?, message_id);
        Ok(())
    }
}
