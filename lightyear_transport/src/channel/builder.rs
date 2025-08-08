//! This module contains the [`Channel`] trait
use crate::channel::receivers::ChannelReceiverEnum;
use crate::channel::registry::{ChannelId, ChannelKind};
use crate::channel::senders::ChannelSend;
use crate::channel::senders::ChannelSenderEnum;
use crate::packet::message::{MessageAck, MessageId};
use crate::packet::packet::PacketId;
use crate::packet::packet_builder::{PacketBuilder, RecvPayload};
use crate::packet::priority_manager::PriorityManager;
use bevy_ecs::component::Component;
use bevy_platform::collections::HashMap;
use bytes::Bytes;
use core::time::Duration;
use lightyear_link::Link;

use crate::channel::Channel;
use crate::error::TransportError;
use crate::prelude::{ChannelRegistry, PriorityConfig};
use crossbeam_channel::{Receiver, Sender};
use lightyear_core::prelude::LocalTimeline;
use lightyear_link::SendPayload;
// TODO: hook when you insert ChannelSettings, it creates a ChannelSender and ChannelReceiver component

use alloc::{vec, vec::Vec};

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
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            mode: ChannelMode::UnorderedUnreliable,
            send_frequency: Duration::default(),
            priority: 1.0,
        }
    }
}

/// Holds information about all the channels present on the entity.
#[derive(Component)]
#[require(LocalTimeline)]
#[require(Link)]
pub struct Transport {
    pub receivers: HashMap<ChannelId, ReceiverMetadata>,
    pub senders: HashMap<ChannelKind, SenderMetadata>,
    /// PriorityManager shared between all channels of this transport
    pub priority_manager: PriorityManager,
    /// PacketBuilder shared between all channels of this transport
    pub(crate) packet_manager: PacketBuilder,

    // TODO: do a HashMap<MessageId, PacketId> instead?
    // - when we receive a packet, go through all messages and check which ones match the packet? there shouldn't be too many
    //   since packets are quickly acked/nacked. We could also have a map from packet_id to message_ids.
    // - if the message is fragmented, we need to ack num_fragment packets to actually receive the ack. Any nack results in a nack
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
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
        Self {
            receivers: Default::default(),
            senders: Default::default(),
            priority_manager: PriorityManager::new(priority_config),
            packet_manager: PacketBuilder::default(),
            packet_to_message_map: Default::default(),
            fragment_acks: Default::default(),
            send_channel,
            recv_channel,
            send: vec![],
            recv: vec![],
        }
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::new(PriorityConfig::default())
    }
}

impl Transport {
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
        sender: ChannelSenderEnum,
        mode: ChannelMode,
        channel_id: ChannelId,
    ) {
        self.senders.insert(
            ChannelKind::of::<C>(),
            SenderMetadata {
                sender,
                message_acks: vec![],
                message_nacks: vec![],
                messages_sent: vec![],
                channel_id,
                mode,
                name: core::any::type_name::<C>(),
            },
        );
    }

    // TODO: make this available via observer by triggering AddSender<C> on the Transport entity.
    pub fn add_sender_from_registry<C: Channel>(&mut self, registry: &ChannelRegistry) {
        let Some(settings) = registry.settings(ChannelKind::of::<C>()) else {
            panic!(
                "ChannelSettings not found for channel {}",
                core::any::type_name::<C>()
            );
        };
        let channel_id = *registry.get_net_from_kind(&ChannelKind::of::<C>()).unwrap();
        let sender = settings.into();
        self.add_sender::<C>(sender, settings.mode, channel_id);
    }

    pub fn add_receiver<C: Channel>(
        &mut self,
        receiver: ChannelReceiverEnum,
        channel_id: ChannelId,
    ) {
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
                core::any::type_name::<C>()
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
        let message_id = sender_metadata.sender.buffer_send(bytes, priority);
        Ok(message_id)
    }

    /// Reset the Transport to a default state upon disconnection
    pub(crate) fn reset(&mut self, registry: &ChannelRegistry) {
        self.receivers.iter_mut().for_each(|(channel_id, r)| {
            let settings = registry.settings_from_net_id(*channel_id).unwrap();
            *r = ReceiverMetadata {
                receiver: settings.into(),
                channel_kind: r.channel_kind,
            };
        });
        self.senders.iter_mut().for_each(|(channel_kind, s)| {
            let settings = registry.settings(*channel_kind).unwrap();
            *s = SenderMetadata {
                sender: settings.into(),
                message_acks: vec![],
                message_nacks: vec![],
                messages_sent: vec![],
                channel_id: s.channel_id,
                mode: s.mode,
                name: s.name,
            };
        });
        self.priority_manager = Default::default();
        self.packet_manager = Default::default();
        self.packet_to_message_map = Default::default();
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
    pub(crate) name: &'static str,
}

// fn on_add<C: Channel>(mut world: DeferredWorld, context: HookContext) {
//     let entity = context.entity;
//     let mut registry = world.resource_mut::<ChannelRegistry>();
//     // TODO: merge settings and SenderId in a SenderMetadata so that we don't fetch twice
//     let Some(settings) = registry.settings::<C>() else {
//         panic!("ChannelSettings not found for channel {}", core::any::type_name::<C>());
//     };
//     let sender_id = registry.get_sender_id::<C>().unwrap();
//     let channel_id = *registry.get_net_from_kind(&ChannelKind::of::<C>()).unwrap();
//     let receiver: ChannelReceiverEnum;
//     let sender: ChannelSenderEnum;
//     let mode = settings.mode;
//     match settings.mode {
//         ChannelMode::UnorderedUnreliableWithAcks => {
//             receiver = UnorderedUnreliableReceiver::new().into();
//             sender = UnorderedUnreliableWithAcksSender::new(settings.send_frequency).into();
//         }
//         ChannelMode::UnorderedUnreliable => {
//             receiver = UnorderedUnreliableReceiver::new().into();
//             sender = UnorderedUnreliableSender::new(settings.send_frequency).into();
//         }
//         ChannelMode::SequencedUnreliable => {
//             receiver = SequencedUnreliableReceiver::new().into();
//             sender = SequencedUnreliableSender::new(settings.send_frequency).into();
//         }
//         ChannelMode::UnorderedReliable(reliable_settings) => {
//             receiver = UnorderedReliableReceiver::new().into();
//             sender = ReliableSender::new(reliable_settings, settings.send_frequency).into();
//         }
//         ChannelMode::SequencedReliable(reliable_settings) => {
//             receiver = SequencedReliableReceiver::new().into();
//             sender = ReliableSender::new(reliable_settings, settings.send_frequency).into();
//         }
//         ChannelMode::OrderedReliable(reliable_settings) => {
//             receiver = OrderedReliableReceiver::new().into();
//             sender = ReliableSender::new(reliable_settings, settings.send_frequency).into();
//         }
//     }
//     let mut entity_mut = world.entity_mut(entity);
//     let mut channel_sender = entity_mut.get_mut::<ChannelSender<C>>().unwrap();
//     channel_sender.sender = sender;
//     channel_sender.channel_id = channel_id;
//     let mut transport = entity_mut.get_mut::<Transport>().unwrap();
//     transport.add_sender::<C>(sender_id, mode);
//     transport.receivers.insert(channel_id, receiver);
// }

#[derive(Clone, Copy, Debug, PartialEq)]
/// ChannelMode specifies how messages are sent and received
/// See more information [here](http://www.jenkinssoftware.com/raknet/manual/reliabilitytypes.html)
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

/// Default channel to send inputs from client to server. This is a Sequenced Unreliable channel.
pub struct InputChannel;

/// Channel to send messages related to Authority transfers
/// This is an Ordered Reliable channel
pub struct AuthorityChannel;
