use alloc::collections::VecDeque;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::platform::collections::HashMap;
use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender};
use tracing::trace;
#[cfg(feature = "trace")]
use tracing::{instrument, Level};
use crate::channel::builder::ChannelContainer;
use crate::channel::receivers::ChannelReceive;
use crate::channel::senders::ChannelSend;
#[cfg(feature = "trace")]
use crate::channel::stats::send::ChannelSendStats;
use crate::packet::error::PacketError;
use crate::packet::header::PacketHeader;
use crate::packet::message::{
    FragmentData, MessageAck, MessageId, ReceiveMessage, SendMessage, SingleData,
};
use crate::packet::packet::PacketId;
use crate::packet::packet_builder::{PacketBuilder, Payload, RecvPayload};
use crate::packet::packet_type::PacketType;
use crate::packet::priority_manager::{PriorityConfig, PriorityManager};
use crate::protocol::channel::{ChannelId, ChannelKind, ChannelRegistry};
use crate::protocol::registry::NetId;
use crate::serialize::reader::{ReadInteger, Reader};
use crate::serialize::writer::Writer;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;
#[cfg(test)]
use crate::utils::captures::Captures;

// TODO: hard to split message manager into send/receive because the acks need both the send side and receive side
//  maybe have a separate actor for acks?

pub const DEFAULT_MESSAGE_PRIORITY: f32 = 1.0;

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
#[derive(Debug)]
pub struct MessageManager {
    pub(crate) writer: Writer,
    /// Handles sending/receiving packets (including acks)
    packet_manager: PacketBuilder,
    priority_manager: PriorityManager,
    pub(crate) channels: HashMap<ChannelKind, ChannelContainer>,
    pub(crate) channel_registry: ChannelRegistry,
    // TODO: can use Vec<ChannelKind, Vec<MessageId>> to be more efficient?
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    packet_to_message_ack_map: HashMap<PacketId, Vec<(ChannelKind, MessageAck)>>,
    nack_senders: Vec<Sender<MessageId>>,
}

impl MessageManager {
    pub fn new(
        channel_registry: &ChannelRegistry,
        nack_rtt_multiple: f32,
        priority_config: PriorityConfig,
    ) -> Self {
        Self {
            writer: Writer::default(),
            packet_manager: PacketBuilder::new(nack_rtt_multiple),
            priority_manager: PriorityManager::new(priority_config),
            channels: channel_registry.channels(),
            channel_registry: channel_registry.clone(),
            packet_to_message_ack_map: HashMap::default(),
            nack_senders: vec![],
        }
    }

    pub(crate) fn get_replication_update_send_receiver(&mut self) -> Receiver<MessageId> {
        self.priority_manager
            .subscribe_replication_update_sent_messages()
    }

    /// Update bookkeeping
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn update(
        &mut self,
        time_manager: &TimeManager,
        ping_manager: &PingManager,
        tick_manager: &TickManager,
    ) {
        // on the sender side, gather the list of packets that haven't been received by the remote peer
        let lost_packets = self
            .packet_manager
            .header_manager
            .update(time_manager, ping_manager);
        // notify that some messages have been lost
        for lost_packet in lost_packets {
            if let Some(message_map) = self.packet_to_message_ack_map.remove(&lost_packet) {
                for (channel_kind, message_ack) in message_map {
                    let channel = self
                        .channels
                        .get_mut(&channel_kind)
                        .expect("Channel not found");
                    // TODO: batch the messages?
                    trace!(
                        ?lost_packet,
                        ?channel_kind,
                        "message lost: {:?}",
                        message_ack.message_id
                    );
                    channel.sender.send_nacks(message_ack.message_id);
                }
            }
        }
        for channel in self.channels.values_mut() {
            channel
                .sender
                .update(time_manager, ping_manager, tick_manager);
            channel.receiver.update(time_manager, tick_manager);
        }
    }

    /// Buffer a message to be sent on this connection
    /// Returns the message id associated with the message, if there is one
    pub fn buffer_send(
        &mut self,
        message: Bytes,
        channel_kind: ChannelKind,
    ) -> Result<Option<MessageId>, PacketError> {
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
    pub fn buffer_send_with_priority(
        &mut self,
        message: Bytes,
        channel_kind: ChannelKind,
        priority: f32,
    ) -> Result<Option<MessageId>, PacketError> {
        let channel = self
            .channels
            .get_mut(&channel_kind)
            .ok_or(PacketError::ChannelNotFound)?;
        Ok(channel.sender.buffer_send(message, priority)?)
    }

    /// Prepare buckets from the internal send buffers, and return the bytes to send
    // TODO: maybe pass TickManager instead of Tick? Find a more elegant way to pass extra data that might not be used?
    //  (ticks are not purely necessary without client prediction)
    //  maybe be generic over a Context ?
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn send_packets(&mut self, current_tick: Tick) -> Result<Vec<Payload>, PacketError> {
        // Step 1. Get the list of packets to send from all channels
        // for each channel, prepare packets using the buffered messages that are ready to be sent
        // TODO: iterate through the channels in order of channel priority? (with accumulation)
        let mut data_to_send: Vec<(NetId, (VecDeque<SendMessage>, VecDeque<SendMessage>))> = vec![];
        let mut has_data_to_send = false;
        for (channel_kind, channel) in self.channels.iter_mut() {
            let channel_id = self
                .channel_registry
                .get_net_from_kind(channel_kind)
                .ok_or(PacketError::ChannelNotFound)?;
            let (single_data, fragment_data) = channel.sender.send_packet();

            if !single_data.is_empty() || !fragment_data.is_empty() {
                trace!(?channel_id, "send message with channel_id");
                data_to_send.push((*channel_id, (single_data, fragment_data)));
                has_data_to_send = true;
            }
        }
        // return early if there are no messages to send
        if !has_data_to_send {
            return Ok(vec![]);
        }

        // priority manager: get the list of messages we can send according to the rate limiter
        //  (the other messages are stored in an internal buffer)
        let (single_data, fragment_data, num_bytes_added_to_limiter) = self
            .priority_manager
            .priority_filter(data_to_send, &self.channel_registry, current_tick);

        #[cfg(feature = "trace")]
        {
            // NOTE: we don't know the actual exact amount of bytes sent (because we don't take into account the ids, etc.),
            // but we could during build_packet?
            for (channel_id, data) in &single_data {
                let channel_stats = &mut self
                    .channels
                    .get_mut(
                        self.channel_registry
                            .get_kind_from_net_id(*channel_id)
                            .ok_or(PacketError::ChannelNotFound)?,
                    )
                    .ok_or(PacketError::ChannelNotFound)?
                    .sender_stats;
                channel_stats.add_bytes_sent(data.iter().fold(0, |acc, d| acc + d.bytes.len()));
                channel_stats.add_single_message_sent(data.len());
            }
            for (channel_id, data) in &fragment_data {
                let channel_stats = &mut self
                    .channels
                    .get_mut(
                        self.channel_registry
                            .get_kind_from_net_id(*channel_id)
                            .ok_or(PacketError::ChannelNotFound)?,
                    )
                    .ok_or(PacketError::ChannelNotFound)?
                    .sender_stats;
                channel_stats.add_bytes_sent(data.iter().fold(0, |acc, d| acc + d.bytes.len()));
                channel_stats.add_fragment_message_sent(data.len());
            }
        }

        let packets =
            self.packet_manager
                .build_packets(current_tick, single_data, fragment_data)?;

        // for packet in packets.iter() {
        //     trace!(?packet, "packet to send");
        // }

        let mut bytes = Vec::new();
        for mut packet in packets {
            trace!(packet_id = ?packet.packet_id, num_messages = ?packet.num_messages(), "sending packet");
            // TODO: should we update this to include fragment info as well?
            // Step 2. Update the packet_to_message_id_map (only for channels that care about acks)
            core::mem::take(&mut packet.message_acks)
                .into_iter()
                .try_for_each(|(channel_id, message_ack)| {
                    let channel_kind = self
                        .channel_registry
                        .get_kind_from_net_id(channel_id)
                        .ok_or(PacketError::ChannelNotFound)?;
                    let channel = self
                        .channels
                        .get(channel_kind)
                        .ok_or(PacketError::ChannelNotFound)?;
                    if channel.setting.mode.is_watching_acks() {
                        trace!(
                            "Registering message ack (ChannelId:{:?} {:?}) for packet {:?}",
                            channel_id,
                            message_ack,
                            packet.packet_id
                        );
                        self.packet_to_message_ack_map
                            .entry(packet.packet_id)
                            .or_default()
                            .push((*channel_kind, message_ack));
                    }
                    Ok::<(), PacketError>(())
                })?;

            // Step 3. Get the packets to send over the network
            bytes.push(packet.payload);
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
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn recv_packet(&mut self, packet: RecvPayload) -> Result<Tick, PacketError> {
        trace!(packet = ?packet.as_ref(), "Received packet");
        let mut cursor = Reader::from(packet);

        // Step 1. Parse the packet
        let header = PacketHeader::from_bytes(&mut cursor)?;
        let tick = header.tick;
        trace!(?header);

        // TODO: if it's fragmented, put it in a buffer? while we wait for all the parts to be ready?
        //  maybe the channel can handle the fragmentation?

        // TODO: an option is to have an async task that is on the receiving side of the
        //  cross-beam channel which tell which packets have been received

        // Step 2. Update the packet acks (which packets have we received, and which of our packets
        // have been acked)
        let acked_packets = self
            .packet_manager
            .header_manager
            .process_recv_packet_header(&header);

        // Step 3. Update the list of messages that have been acked
        for acked_packet in acked_packets {
            trace!("Acked packet {:?}", acked_packet);
            if let Some(message_acks) = self.packet_to_message_ack_map.remove(&acked_packet) {
                for (channel_kind, message_ack) in message_acks {
                    let channel_name = self
                        .channel_registry
                        .name(&channel_kind)
                        .ok_or(PacketError::ChannelNotFound)?;
                    trace!(
                        "Acked message in packet: channel={:?},message_ack={:?}",
                        channel_name,
                        message_ack
                    );
                    let channel = self
                        .channels
                        .get_mut(&channel_kind)
                        .ok_or(PacketError::ChannelNotFound)?;
                    channel.sender.receive_ack(&message_ack);
                }
            }
        }

        // Step 4. Parse the payload into messages, put them in the internal buffers for each channel
        // we read directly from the packet and don't create intermediary datastructures to avoid allocations
        // TODO: maybe do this in a helper function?
        if header.get_packet_type() == PacketType::DataFragment {
            // read the fragment data
            let channel_id = ChannelId::from_bytes(&mut cursor)?;
            let fragment_data = FragmentData::from_bytes(&mut cursor)?;
            self.get_channel_mut(channel_id)?
                .receiver
                .buffer_recv(ReceiveMessage {
                    data: fragment_data.into(),
                    remote_sent_tick: tick,
                })?;
        }
        // read single message data
        while cursor.has_remaining() {
            let channel_id = ChannelId::from_bytes(&mut cursor)?;
            let num_messages = cursor.read_u8().map_err(SerializationError::from)?;
            trace!(?channel_id, ?num_messages);
            for i in 0..num_messages {
                let single_data = SingleData::from_bytes(&mut cursor)?;
                self.get_channel_mut(channel_id)?
                    .receiver
                    .buffer_recv(ReceiveMessage {
                        data: single_data.into(),
                        remote_sent_tick: tick,
                    })?;
            }
        }
        // trace!(
        //         "received {:?} messages from channel: {:?}",
        //         messages,
        //         channel_kind
        //     );
        // TODO: use channel_id 0 as end of packet or just check that we are at the end of the packet?
        Ok(tick)
    }

    /// Read all the messages in the internal buffers that are ready to be processed
    ///
    /// Returns a map of channel kind to a list of messages, along with the sender tick
    /// at which the message was sent.
    ///
    /// CAREFUL: this doesn't mean that the message was buffered at that tick?
    /// (because of prioritization, or because of sender channel buffering)
    /// If that information is needed, you need to include it in the message itself.
    ///
    /// EDIT: Actually, prioritization discards messages that are not sent, so maybe it is guaranteed that the tick
    /// is the remote send tick.
    ///
    /// Right now we cannot use this because we need to call a while loop inside the iterator...
    /// We could have the read_message() return a Box<dyn Iterator>, but let's just copy-paste the code right now.
    #[cfg(test)]
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn read_messages(
        &mut self,
    ) -> impl Iterator<Item = (ChannelKind, (Tick, Bytes))> + Captures<&()> {
        self.channels
            .iter_mut()
            .flat_map(move |(channel_kind, channel)| {
                // TODO: this is broken, we need to call a read_message in a while loop !
                channel.receiver.read_message().map(move |(tick, bytes)| {
                    trace!(?channel_kind, "reading message: {:?}", bytes);
                    // SAFETY: when we receive the message, we set the tick of the message to the header tick
                    // so every message has a tick
                    (*channel_kind, (tick, bytes))
                })
            })
    }

    #[cfg(test)]
    pub fn collect_messages(
        messages: impl Iterator<Item = (ChannelKind, (Tick, Bytes))>,
    ) -> HashMap<ChannelKind, Vec<(Tick, Bytes)>> {
        let mut map = HashMap::default();
        for (channel_kind, (tick, bytes)) in messages {
            map.entry(channel_kind)
                .or_insert_with(Vec::new)
                .push((tick, bytes));
        }
        map
    }

    pub fn get_channel_mut(
        &mut self,
        channel_id: ChannelId,
    ) -> Result<&mut ChannelContainer, PacketError> {
        let channel_kind = self
            .channel_registry
            .get_kind_from_net_id(channel_id)
            .ok_or(PacketError::ChannelNotFound)?;
        self.channels
            .get_mut(channel_kind)
            .ok_or(PacketError::ChannelNotFound)
    }

    /// Get the ChannelSendStats of a given channel
    #[cfg(feature = "trace")]
    pub fn channel_send_stats<C: crate::prelude::Channel>(&self) -> Option<&ChannelSendStats> {
        self.channels
            .get(&ChannelKind::of::<C>())
            .map(|channel| &channel.sender_stats)
    }
}

// TODO: have a way to update the channels about the messages that have been acked

#[cfg(test)]
mod tests {
    use bevy::prelude::default;

    use crate::packet::message::MessageId;
    use crate::packet::packet::FRAGMENT_SIZE;
    use crate::packet::priority_manager::PriorityConfig;
    use crate::prelude::*;

    use crate::tests::protocol::*;

    use super::*;

    fn setup() -> (MessageManager, MessageManager) {
        let mut channel_registry = ChannelRegistry::default();
        channel_registry.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        });
        channel_registry.add_channel::<Channel2>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliableWithAcks,
            ..default()
        });

        // Create message managers
        let client_message_manager =
            MessageManager::new(&channel_registry, 1.5, PriorityConfig::default());
        let server_message_manager =
            MessageManager::new(&channel_registry, 1.5, PriorityConfig::default());
        (client_message_manager, server_message_manager)
    }

    #[test]
    /// We want to test that we can send/receive messages over a connection
    fn test_message_manager_single_message() -> Result<(), PacketError> {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::TRACE)
        //     .init();
        let (mut client_message_manager, mut server_message_manager) = setup();

        // client: buffer send messages, and then send
        let message: Bytes = vec![0, 1].into();
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        client_message_manager.buffer_send(message.clone(), channel_kind_1)?;
        client_message_manager.buffer_send(message.clone(), channel_kind_2)?;
        let payloads = client_message_manager.send_packets(Tick(0))?;
        assert_eq!(
            client_message_manager.packet_to_message_ack_map,
            HashMap::from_iter([(
                PacketId(0),
                vec![(
                    channel_kind_2,
                    MessageAck {
                        message_id: MessageId(0),
                        fragment_id: None,
                    }
                )]
            )])
        );

        // server: receive bytes from the sent messages, then process them into messages
        for payload in payloads {
            server_message_manager.recv_packet(payload.into())?;
        }
        let it = server_message_manager.read_messages();
        let data = MessageManager::collect_messages(it);

        assert_eq!(
            data.get(&channel_kind_1).unwrap(),
            &vec![(Tick(0), message.clone())]
        );
        assert_eq!(
            data.get(&channel_kind_2).unwrap(),
            &vec![(Tick(0), message.clone())]
        );

        // Confirm what happens if we try to receive but there is nothing on the io
        let it = server_message_manager.read_messages();
        let data = MessageManager::collect_messages(it);
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
        let payloads = server_message_manager.send_packets(Tick(0))?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for payload in payloads {
            client_message_manager.recv_packet(payload.into())?;
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
    fn test_message_manager_fragment_message() -> Result<(), PacketError> {
        let (mut client_message_manager, mut server_message_manager) = setup();

        // client: buffer send messages, and then send
        const MESSAGE_SIZE: usize = (1.5 * FRAGMENT_SIZE as f32) as usize;

        let message = Bytes::copy_from_slice(&[0; MESSAGE_SIZE]);
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        client_message_manager.buffer_send(message.clone(), channel_kind_1)?;
        client_message_manager.buffer_send(message.clone(), channel_kind_2)?;
        let payloads = client_message_manager.send_packets(Tick(0))?;
        assert_eq!(payloads.len(), 4);
        // the order of the packets is not guaranteed
        let packets_with_acks = client_message_manager
            .packet_to_message_ack_map
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let acks: Vec<_> = client_message_manager
            .packet_to_message_ack_map
            .clone()
            .into_values()
            .collect();
        assert!(acks.contains(&vec![(
            channel_kind_2,
            MessageAck {
                message_id: MessageId(0),
                fragment_id: Some(0),
            }
        )]));
        assert!(acks.contains(&vec![(
            channel_kind_2,
            MessageAck {
                message_id: MessageId(0),
                fragment_id: Some(1),
            }
        )]));

        // server: receive bytes from the sent messages, then process them into messages
        for payload in payloads {
            server_message_manager.recv_packet(payload.into())?;
        }
        let it = server_message_manager.read_messages();
        let data = MessageManager::collect_messages(it);
        assert_eq!(
            data.get(&channel_kind_1).unwrap(),
            &vec![(Tick(0), message.clone())]
        );
        assert_eq!(
            data.get(&channel_kind_2).unwrap(),
            &vec![(Tick(0), message.clone())]
        );

        // Confirm what happens if we try to receive but there is nothing on the io
        let it = server_message_manager.read_messages();
        let data = MessageManager::collect_messages(it);
        assert!(data.is_empty());

        // Check the state of the packet headers
        assert_eq!(
            client_message_manager
                .packet_manager
                .header_manager
                .next_packet_id(),
            PacketId(4)
        );
        for packet in packets_with_acks {
            assert!(client_message_manager
                .packet_manager
                .header_manager
                .sent_packets_not_acked()
                .contains_key(&packet));
        }

        // Server sends back a message
        server_message_manager.buffer_send(vec![1].into(), channel_kind_1)?;
        let payloads = server_message_manager.send_packets(Tick(0))?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for payload in payloads {
            client_message_manager.recv_packet(payload.into())?;
        }

        // Check that reliability works correctly
        assert_eq!(client_message_manager.packet_to_message_ack_map.len(), 0);
        // TODO: check that client_channel_1's sender's unacked messages is empty
        // let client_channel_1 = client_connection.channels.get(&channel_kind_1).unwrap();
        // assert_eq!(client_channel_1.sender.)
        Ok(())
    }

    #[test]
    fn test_notify_ack() -> Result<(), PacketError> {
        let (mut client_message_manager, mut server_message_manager) = setup();

        let update_acks_tracker = client_message_manager
            .channels
            .get_mut(&ChannelKind::of::<Channel2>())
            .unwrap()
            .sender
            .subscribe_acks();

        let message_id = client_message_manager
            .buffer_send(vec![0].into(), Channel2::kind())?
            .unwrap();
        assert_eq!(message_id, MessageId(0));
        let payloads = client_message_manager.send_packets(Tick(0))?;
        assert_eq!(
            client_message_manager.packet_to_message_ack_map,
            HashMap::from_iter([(
                PacketId(0),
                vec![(
                    Channel2::kind(),
                    MessageAck {
                        message_id,
                        fragment_id: None,
                    }
                )]
            )])
        );

        // server: receive bytes from the sent messages, then process them into messages
        for payload in payloads {
            server_message_manager.recv_packet(payload.into())?;
        }

        // Server sends back a message (to ack the message)
        server_message_manager.buffer_send(vec![1].into(), Channel2::kind())?;
        let payloads = server_message_manager.send_packets(Tick(0))?;

        // On client side: keep looping to receive bytes on the network, then process them into messages
        for payload in payloads {
            client_message_manager.recv_packet(payload.into())?;
        }

        assert_eq!(update_acks_tracker.try_recv().unwrap(), message_id);
        Ok(())
    }
}
