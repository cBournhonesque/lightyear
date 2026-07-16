//! This module contains the [`Channel`] trait
use crate::channel::receivers::{ChannelReceive, ChannelReceiverEnum};
use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::ChannelSend;
use crate::channel::senders::ChannelSenderEnum;
use crate::packet::compression::CompressionConfig;
use crate::packet::error::PacketError;
use crate::packet::message::{MessageAck, MessageId};
use crate::packet::packet::{FRAGMENT_SIZE, MIN_PACKET_SIZE, PacketId, fragment_size_for_mtu};
use crate::packet::packet_builder::{PacketBuilder, RecvPayload};
use crate::packet::priority_manager::{BandwidthLimiter, PriorityManager};
use bevy_ecs::component::Component;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::world::DeferredWorld;
use bevy_platform::collections::HashMap;
use bytes::Bytes;
use core::time::Duration;
use lightyear_link::{DEFAULT_MTU, Link, LinkMtu};
#[allow(unused_imports)]
use tracing::trace;

use crate::channel::Channel;
use crate::error::TransportError;
use crate::prelude::{ChannelRegistry, PriorityConfig};
use crossbeam_channel::{Receiver, Sender};
use lightyear_link::SendPayload;
// TODO: hook when you insert ChannelSettings, it creates a ChannelSender and ChannelReceiver component

use alloc::{vec, vec::Vec};
use bevy_utils::prelude::DebugName;

pub const DEFAULT_MESSAGE_PRIORITY: f32 = 1.0;

/// [`ChannelSettings`] are used to specify how the [`Channel`] behaves (reliability, ordering, direction)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelSettings {
    /// The ordering and reliability guarantees of the channel.
    pub mode: ChannelMode,
    /// How often should we try to send messages on this channel.
    /// Set to `Duration::default()` to send messages every frame if possible.
    pub send_frequency: Duration,
    /// Sets the priority of the channel. The priority is used to choose which bytes to send when we don't have enough
    /// bandwidth to send all bytes. The bytes will be sent in order of highest priority to lowest priority.
    /// The final priority of a message will be `MessagePriority * ChannelPriority`
    ///
    /// See [`PriorityManager`] for more information.
    pub priority: f32,
    /// Whether unreliable messages that could not be admitted by the bandwidth limiter should
    /// remain queued for a later send flush.
    ///
    /// This only controls local bandwidth admission. Once a message enters `Link.send`, network
    /// retransmission behavior is determined by [`ChannelMode`]. Reliable channels always retain
    /// locally unsent messages regardless of this setting. If any fragment of an unreliable
    /// message has already been admitted, its remaining fragments are also retained to avoid
    /// guaranteeing an incomplete local send.
    pub retry_unsent_messages: bool,
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            mode: ChannelMode::UnorderedUnreliable,
            send_frequency: Duration::default(),
            priority: 1.0,
            retry_unsent_messages: true,
        }
    }
}

/// Holds information about all the channels present on the entity.
#[derive(Component)]
#[component(on_add = Transport::on_add)]
#[require(Link)]
pub struct Transport {
    pub receivers: HashMap<ChannelId, ReceiverMetadata>,
    pub senders: HashMap<ChannelKind, SenderMetadata>,
    /// PriorityManager shared between all channels of this transport
    pub priority_manager: PriorityManager,
    /// Bandwidth admission is separate from priority ordering and uses final packet bytes.
    pub(crate) bandwidth_limiter: BandwidthLimiter,
    /// PacketBuilder shared between all channels of this transport
    pub(crate) packet_manager: PacketBuilder,
    /// Stable fragment payload size derived from the link's minimum MTU.
    fragment_size: usize,
    /// Last current link MTU applied to packet quota bursts.
    configured_mtu: usize,
    pub compression: CompressionConfig,

    // TODO: do a HashMap<MessageId, PacketId> instead?
    // - when we receive a packet, go through all messages and check which ones match the packet? there shouldn't be too many
    //   since packets are quickly acked/nacked. We could also have a map from packet_id to message_ids.
    // - if the message is fragmented, we need to ack num_fragment packets to actually receive the ack. Any nack results in a nack
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    ///
    /// Every packet is either acked or nacked, so this shouldn't grow indefinitely
    pub(crate) packet_to_message_map: HashMap<PacketId, Vec<(ChannelKind, MessageAck)>>,
    /// For fragmented messages, we only ack if we acked the packets of all fragments.
    /// This counter keeps track of the number of packet acks remaining before we can ack the message.
    pub(crate) fragment_acks: HashMap<MessageId, u64>,

    /// mpsc channel sender/receiver to allow users to write bytes to the same channel in parallel
    pub send_channel: Sender<(ChannelKind, Bytes, f32)>,
    pub recv_channel: Receiver<(ChannelKind, Bytes, f32)>,
    /// Buffer to store payloads that have been processed by the transport, and will be processed
    /// by the Link or the Connection
    pub send: Vec<SendPayload>,
    /// Buffer to store payloads that will be processed by the transport and stored in the ChannelReceiverEnum
    pub recv: Vec<RecvPayload>,
}

impl Transport {
    pub fn new(priority_config: PriorityConfig) -> Self {
        let (send_channel, recv_channel) = crossbeam_channel::unbounded();
        let bandwidth_limiter = BandwidthLimiter::new(priority_config.clone(), DEFAULT_MTU);
        Self {
            receivers: Default::default(),
            senders: Default::default(),
            priority_manager: PriorityManager::new(priority_config),
            bandwidth_limiter,
            packet_manager: PacketBuilder::default(),
            fragment_size: FRAGMENT_SIZE,
            configured_mtu: DEFAULT_MTU,
            compression: CompressionConfig::default(),
            packet_to_message_map: Default::default(),
            fragment_acks: Default::default(),
            send_channel,
            recv_channel,
            send: vec![],
            recv: vec![],
        }
    }

    pub fn with_compression(mut self, compression: CompressionConfig) -> Self {
        self.compression = compression;
        self
    }

    pub fn set_compression(&mut self, compression: CompressionConfig) {
        self.compression = compression;
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::new(PriorityConfig::default())
    }
}

impl Transport {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let Some(link_mtu) = world.get::<Link>(context.entity).map(|link| link.mtu) else {
            return;
        };
        let Some(mut transport) = world.get_mut::<Transport>(context.entity) else {
            return;
        };
        if let Err(error) = transport.configure_link_mtu(link_mtu) {
            tracing::error!(?error, "invalid link MTU for transport");
        }
    }

    pub(crate) fn configure_link_mtu(&mut self, link_mtu: LinkMtu) -> Result<(), PacketError> {
        let min_mtu = link_mtu.min_mtu();
        let fragment_size = fragment_size_for_mtu(min_mtu).ok_or(PacketError::MtuTooSmall {
            actual: min_mtu,
            min: MIN_PACKET_SIZE,
        })?;
        if self.fragment_size != fragment_size {
            self.fragment_size = fragment_size;
            self.senders
                .values_mut()
                .for_each(|metadata| metadata.sender.set_fragment_size(fragment_size));
            self.receivers
                .values_mut()
                .for_each(|metadata| metadata.receiver.set_fragment_size(fragment_size));
        }

        let current_mtu = link_mtu.mtu();
        if self.configured_mtu != current_mtu {
            self.configured_mtu = current_mtu;
            self.bandwidth_limiter =
                BandwidthLimiter::new(self.priority_manager.config.clone(), current_mtu);
        }
        Ok(())
    }

    pub fn has_sender<C: Channel>(&self) -> bool {
        self.senders.contains_key(&ChannelKind::of::<C>())
    }

    pub fn has_receiver<C: Channel>(&self) -> bool {
        self.receivers
            .values()
            .any(|m| m.channel_kind == ChannelKind::of::<C>())
    }

    pub fn add_sender<C: Channel>(
        &mut self,
        mut sender: ChannelSenderEnum,
        mode: ChannelMode,
        channel_id: ChannelId,
    ) {
        sender.set_fragment_size(self.fragment_size);
        self.senders.insert(
            ChannelKind::of::<C>(),
            SenderMetadata {
                sender,
                message_acks: vec![],
                message_nacks: vec![],
                messages_sent: vec![],
                channel_id,
                mode,
                name: DebugName::type_name::<C>(),
            },
        );
    }

    // TODO: make this available via observer by triggering AddSender<C> on the Transport entity.
    pub fn add_sender_from_registry<C: Channel>(&mut self, registry: &ChannelRegistry) {
        trace!(
            "Adding sender from registry for channel {}. Kind: {:?}",
            DebugName::type_name::<C>(),
            ChannelKind::of::<C>()
        );
        let Some(settings) = registry.settings(ChannelKind::of::<C>()) else {
            panic!(
                "ChannelSettings not found for channel {}",
                DebugName::type_name::<C>()
            );
        };
        let channel_id = *registry.get_net_from_kind(&ChannelKind::of::<C>()).unwrap();
        let sender = settings.into();
        self.add_sender::<C>(sender, settings.mode, channel_id);
    }

    pub fn add_receiver<C: Channel>(
        &mut self,
        mut receiver: ChannelReceiverEnum,
        channel_id: ChannelId,
    ) {
        receiver.set_fragment_size(self.fragment_size);
        self.receivers.insert(
            channel_id,
            ReceiverMetadata {
                receiver,
                channel_kind: ChannelKind::of::<C>(),
            },
        );
    }

    pub fn add_receiver_from_registry<C: Channel>(&mut self, registry: &ChannelRegistry) {
        let Some(settings) = registry.settings(ChannelKind::of::<C>()) else {
            panic!(
                "ChannelSettings not found for channel {}",
                DebugName::type_name::<C>()
            );
        };
        let channel_id = *registry.get_net_from_kind(&ChannelKind::of::<C>()).unwrap();
        let receiver = settings.into();
        self.add_receiver::<C>(receiver, channel_id);
    }

    pub fn send_with_priority<C: Channel>(
        &self,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<(), TransportError> {
        self.send_erased(ChannelKind::of::<C>(), bytes, priority)
    }

    pub fn send<C: Channel>(&self, bytes: SendPayload) -> Result<(), TransportError> {
        self.send_with_priority::<C>(bytes, 1.0)
    }

    pub fn send_erased(
        &self,
        kind: ChannelKind,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<(), TransportError> {
        self.send_channel.try_send((kind, bytes, priority))?;
        Ok(())
    }

    pub fn send_mut<C: Channel>(
        &mut self,
        bytes: SendPayload,
    ) -> Result<Option<MessageId>, TransportError> {
        self.send_mut_with_priority::<C>(bytes, 1.0)
    }

    pub fn send_mut_with_priority<C: Channel>(
        &mut self,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<Option<MessageId>, TransportError> {
        self.send_mut_erased(ChannelKind::of::<C>(), bytes, priority)
    }

    pub fn send_mut_erased(
        &mut self,
        kind: ChannelKind,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<Option<MessageId>, TransportError> {
        let sender_metadata = self
            .senders
            .get_mut(&kind)
            .ok_or(TransportError::ChannelNotFound(kind))?;
        let message_id = sender_metadata
            .sender
            .buffer_send(bytes, priority, self.compression);
        Ok(message_id)
    }

    /// Reset the Transport to a default state upon disconnection
    pub(crate) fn reset(&mut self, registry: &ChannelRegistry) {
        self.receivers.iter_mut().for_each(|(channel_id, r)| {
            let settings = registry.settings_from_net_id(*channel_id).unwrap();
            let mut receiver: ChannelReceiverEnum = settings.into();
            receiver.set_fragment_size(self.fragment_size);
            *r = ReceiverMetadata {
                receiver,
                channel_kind: r.channel_kind,
            };
        });
        self.senders.iter_mut().for_each(|(channel_kind, s)| {
            let settings = registry.settings(*channel_kind).unwrap();
            let mut sender: ChannelSenderEnum = settings.into();
            sender.set_fragment_size(self.fragment_size);
            *s = SenderMetadata {
                sender,
                message_acks: vec![],
                message_nacks: vec![],
                messages_sent: vec![],
                channel_id: s.channel_id,
                mode: s.mode,
                name: s.name.clone(),
            };
        });
        let priority_config = self.priority_manager.config.clone();
        self.priority_manager = PriorityManager::new(priority_config.clone());
        self.bandwidth_limiter = BandwidthLimiter::new(priority_config, self.configured_mtu);
        self.packet_manager = Default::default();
        self.packet_to_message_map = Default::default();
        self.fragment_acks.clear();
        let (send_channel, recv_channel) = crossbeam_channel::unbounded();
        self.send_channel = send_channel;
        self.recv_channel = recv_channel;
        self.recv.clear();
        self.send.clear();
    }
}

pub struct ReceiverMetadata {
    pub receiver: ChannelReceiverEnum,
    pub channel_kind: ChannelKind,
}

#[doc(hidden)]
pub struct SenderMetadata {
    /// The component id of the ChannelSender<C> component
    pub sender: ChannelSenderEnum,
    // TODO: these are currently only used by EntityUpdatesChannel. Maybe limit their computation only to that channel?
    /// List of messages that have been acked; is cleared every frame.
    pub message_acks: Vec<MessageId>,
    /// List of messages that have been nacked; is cleared every frame.
    pub message_nacks: Vec<MessageId>,
    /// List of messages that have been sent; is cleared every frame. Note that buffering a message via ChannelSender::send does
    /// not guarantee that the message will actually be sent, because of the PriorityManager.
    pub messages_sent: Vec<MessageId>,
    pub(crate) channel_id: ChannelId,
    pub(crate) mode: ChannelMode,
    pub(crate) name: DebugName,
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// ChannelMode specifies how messages are sent and received
/// See more information [here](https://web.archive.org/web/20250113064633/http://www.jenkinssoftware.com/raknet/manual/reliabilitytypes.html)
pub enum ChannelMode {
    /// Messages may arrive out-of-order, or not at all.
    /// Still keep track of which messages got received.
    UnorderedUnreliableWithAcks,
    /// Messages may arrive out-of-order, or not at all
    UnorderedUnreliable,
    /// Same as unordered unreliable, but only the newest message is ever accepted, older messages
    /// are ignored
    SequencedUnreliable,
    /// Messages may arrive out-of-order, but we make sure (with retries, acks) that the message
    /// will arrive
    UnorderedReliable(ReliableSettings),
    /// Same as unordered reliable, but the messages are sequenced (only the newest message is accepted)
    SequencedReliable(ReliableSettings),
    /// Messages will arrive in the correct order at the destination
    OrderedReliable(ReliableSettings),
}

impl ChannelMode {
    pub fn is_reliable(&self) -> bool {
        match self {
            ChannelMode::UnorderedUnreliableWithAcks => false,
            ChannelMode::UnorderedUnreliable => false,
            ChannelMode::SequencedUnreliable => false,
            ChannelMode::UnorderedReliable(_) => true,
            ChannelMode::SequencedReliable(_) => true,
            ChannelMode::OrderedReliable(_) => true,
        }
    }

    /// Returns true if the channel cares about tracking ACKs of messages
    pub(crate) fn is_watching_acks(&self) -> bool {
        match self {
            ChannelMode::UnorderedUnreliableWithAcks => true,
            ChannelMode::UnorderedUnreliable => false,
            ChannelMode::SequencedUnreliable => false,
            ChannelMode::UnorderedReliable(_) => true,
            ChannelMode::SequencedReliable(_) => true,
            ChannelMode::OrderedReliable(_) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReliableSettings {
    /// Multiplier of the current RTT estimate, used for delay to wait before resending a packet if it has not been acked.
    pub rtt_resend_factor: f32,
    /// Minimum duration to wait before resending a packet if it has not been acked
    pub rtt_resend_min_delay: Duration,
}

impl Default for ReliableSettings {
    fn default() -> Self {
        Self {
            rtt_resend_factor: 1.5,
            rtt_resend_min_delay: Duration::default(),
        }
    }
}

impl ReliableSettings {
    pub(crate) fn resend_delay(&self, rtt: Duration) -> Duration {
        let delay = rtt.mul_f32(self.rtt_resend_factor);
        core::cmp::max(delay, self.rtt_resend_min_delay)
    }
}

#[cfg(all(test, feature = "compression_lz4"))]
mod tests {
    use super::*;
    use crate::channel::receivers::ChannelReceive;
    use crate::packet::compression::decompress_payload;
    use crate::packet::error::PacketError;
    use crate::packet::header::PacketHeader;
    use crate::packet::message::{
        FragmentCompression, FragmentData, MessageData, ReceiveMessage, SingleData,
    };
    use crate::packet::packet::{FRAGMENT_SIZE, Packet};
    use crate::packet::packet_type::PacketType;
    use bytes::Bytes;
    use lightyear_core::tick::Tick;
    use lightyear_serde::reader::{ReadInteger, Reader};
    use lightyear_serde::{SerializationError, ToBytes};

    struct CompressionChannel;

    #[test]
    fn compressed_fragmented_message_round_trips_through_sender_packet_builder_and_receiver()
    -> Result<(), PacketError> {
        let compression = CompressionConfig {
            min_payload_size: 0,
            max_decompressed_payload_size: FRAGMENT_SIZE * 4,
            ..CompressionConfig::LZ4
        };
        let settings = ChannelSettings {
            mode: ChannelMode::SequencedUnreliable,
            ..ChannelSettings::default()
        };
        let mut registry = ChannelRegistry::default();
        let (channel_kind, channel_id) = registry.add_channel::<CompressionChannel>(settings);

        let mut sender_transport =
            Transport::new(PriorityConfig::default()).with_compression(compression);
        sender_transport.add_sender::<CompressionChannel>(
            (&settings).into(),
            settings.mode,
            channel_id,
        );

        let mut receiver_transport =
            Transport::new(PriorityConfig::default()).with_compression(compression);
        receiver_transport.add_receiver::<CompressionChannel>((&settings).into(), channel_id);

        let message = Bytes::from(vec![7u8; FRAGMENT_SIZE * 3]);
        let message_id = sender_transport
            .send_mut_erased(channel_kind, message.clone(), 1.0)
            .unwrap()
            .unwrap();

        let mut candidates = vec![];
        sender_transport
            .senders
            .get_mut(&channel_kind)
            .unwrap()
            .sender
            .collect_send_candidates(channel_kind, channel_id, &mut candidates);
        assert!(!candidates.is_empty());

        let fragments = candidates
            .iter()
            .map(|candidate| match &candidate.message.data {
                MessageData::Fragment(fragment) => fragment,
                MessageData::Single(_) => panic!("oversized message should be fragmented"),
            })
            .collect::<Vec<_>>();
        assert!(fragments.len() < 3);
        assert_eq!(fragments[0].compression, Some(FragmentCompression::Lz4));
        assert!(
            fragments
                .iter()
                .skip(1)
                .all(|fragment| fragment.compression.is_none())
        );

        let mut cursor = crate::packet::packet_builder::CandidateCursor::default();
        let mut packets = vec![];
        while let Some(packet) = sender_transport.packet_manager.build_next_packet(
            Tick(0),
            &candidates,
            &mut cursor,
            compression,
            DEFAULT_MTU,
        )? {
            sender_transport
                .packet_manager
                .header_manager
                .commit_send_packet(packet.packet_id, Duration::default());
            packets.push(packet);
        }
        assert!(!packets.is_empty());

        for packet in packets {
            buffer_packet_into_receivers(&mut receiver_transport, packet, compression)?;
        }

        let receiver = &mut receiver_transport
            .receivers
            .get_mut(&channel_id)
            .unwrap()
            .receiver;
        assert_eq!(
            receiver.read_message(),
            Some((Tick(0), message, Some(message_id)))
        );
        Ok(())
    }

    #[test]
    fn bandwidth_limiter_uses_compressed_packet_size_after_packing() -> Result<(), PacketError> {
        let compression = CompressionConfig {
            min_payload_size: 0,
            ..CompressionConfig::LZ4
        };
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliableWithAcks,
            ..ChannelSettings::default()
        };
        let mut registry = ChannelRegistry::default();
        let (channel_kind, channel_id) = registry.add_channel::<CompressionChannel>(settings);
        let mut transport = Transport::new(PriorityConfig::new(600)).with_compression(compression);
        transport.add_sender::<CompressionChannel>((&settings).into(), settings.mode, channel_id);

        for _ in 0..8 {
            transport
                .send_mut_erased(channel_kind, Bytes::from(vec![5u8; 200]), 1.0)
                .unwrap();
        }

        let candidates = transport.priority_manager.candidates_mut();
        transport
            .senders
            .get_mut(&channel_kind)
            .unwrap()
            .sender
            .collect_send_candidates(channel_kind, channel_id, candidates);
        assert_eq!(transport.priority_manager.candidates().len(), 8);
        transport.priority_manager.prioritize(&registry);
        let mut cursor = crate::packet::packet_builder::CandidateCursor::default();
        let packet = transport
            .packet_manager
            .build_next_packet(
                Tick(0),
                transport.priority_manager.candidates(),
                &mut cursor,
                compression,
                DEFAULT_MTU,
            )?
            .unwrap();

        assert_eq!(packet.num_messages(), 8);
        assert!(packet.compression.is_some());
        assert!(packet.payload.len() <= 600);
        assert!(
            transport
                .bandwidth_limiter
                .consume_packet_quota(packet.payload.len())
        );
        Ok(())
    }

    fn buffer_packet_into_receivers(
        transport: &mut Transport,
        packet: Packet,
        compression: CompressionConfig,
    ) -> Result<(), PacketError> {
        let mut cursor = Reader::from(packet.payload);
        let header = PacketHeader::from_bytes(&mut cursor)?;
        let tick = header.tick;
        let mut packet_type = header.get_packet_type();
        if packet_type.is_compressed() {
            let compressed_payload = cursor.split();
            let decompressed_payload =
                decompress_payload(compressed_payload.as_ref(), compression)?;
            cursor = Reader::from(decompressed_payload);
            packet_type = packet_type.uncompressed_variant();
        }

        if packet_type == PacketType::DataFragment {
            let channel_id = ChannelId::from_bytes(&mut cursor)?;
            let fragment_data = FragmentData::from_bytes(&mut cursor)?;
            transport
                .receivers
                .get_mut(&channel_id)
                .ok_or(PacketError::ChannelNotFound)?
                .receiver
                .buffer_recv(ReceiveMessage {
                    data: fragment_data.into(),
                    remote_sent_tick: tick,
                    compression,
                })?;
        }

        while cursor.has_remaining() {
            let channel_id = ChannelId::from_bytes(&mut cursor)?;
            let num_messages = cursor.read_u8().map_err(SerializationError::from)?;
            for _ in 0..num_messages {
                let single_data = SingleData::from_bytes(&mut cursor)?;
                transport
                    .receivers
                    .get_mut(&channel_id)
                    .ok_or(PacketError::ChannelNotFound)?
                    .receiver
                    .buffer_recv(ReceiveMessage {
                        data: single_data.into(),
                        remote_sent_tick: tick,
                        compression,
                    })?;
            }
        }
        Ok(())
    }
}
