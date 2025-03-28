//! This module contains the [`Channel`] trait
use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
use crate::channel::receivers::sequenced_reliable::SequencedReliableReceiver;
use crate::channel::receivers::sequenced_unreliable::SequencedUnreliableReceiver;
use crate::channel::receivers::unordered_reliable::UnorderedReliableReceiver;
use crate::channel::receivers::unordered_unreliable::UnorderedUnreliableReceiver;
use crate::channel::receivers::ChannelReceiverEnum;
use crate::channel::registry::{ChannelId, ChannelKind, ChannelRegistry};
use crate::channel::senders::reliable::ReliableSender;
use crate::channel::senders::sequenced_unreliable::SequencedUnreliableSender;
use crate::channel::senders::unordered_unreliable::UnorderedUnreliableSender;
use crate::channel::senders::unordered_unreliable_with_acks::UnorderedUnreliableWithAcksSender;
use crate::channel::senders::ChannelSend;
use crate::channel::senders::ChannelSenderEnum;
#[cfg(feature = "trace")]
use crate::channel::stats::send::ChannelSendStats;
use crate::packet::message::{MessageAck, MessageId};
use crate::packet::packet::PacketId;
use crate::packet::packet_builder::{PacketBuilder, RecvPayload};
use crate::packet::priority_manager::PriorityManager;
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::component::{ComponentHook, ComponentId, ComponentsRegistrator, HookContext, Immutable, RequiredComponents, StorageType};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, World};
use bytes::Bytes;
use core::time::Duration;
use lightyear_macros::{Channel, ChannelInternal};
use lightyear_serde::SerializationError;
use lightyear_utils::collections::HashMap;

use crate::channel::Channel;
use crate::entity_map::SendEntityMap;
use alloc::collections::VecDeque;
use lightyear_link::SendPayload;
use lightyear_serde::writer::Writer;
use tracing::trace;
// TODO: hook when you insert ChannelSettings, it creates a ChannelSender and ChannelReceiver component

pub const DEFAULT_MESSAGE_PRIORITY: f32 = 1.0;

/// [`ChannelSettings`] are used to specify how the [`Channel`] behaves (reliability, ordering, direction)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelSettings {
    pub mode: ChannelMode,
    /// How often should we try to send messages on this channel.
    /// Set to `Duration::default()` to send messages every frame if possible.
    pub send_frequency: Duration,
    /// Sets the priority of the channel. The final priority of a message will be `MessagePriority * ChannelPriority`
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


/// User-facing component to send data to a given channel.
///
/// We create a separate component per channel so that users can buffer messages to one channel
/// without losing parallelism over the other channels.
#[derive(Component)]
#[require(Transport)]
// TODO: add on_remove or on_replace hooks?
#[component(on_add = on_add::<C>)]
pub struct ChannelSender<C: Channel> {
    pub sender: ChannelSenderEnum,
    pub channel_id: ChannelId,
    pub writer: Writer,
    marker: std::marker::PhantomData<C>,
}

// The user will just provide the Default ChannelSender and we will use the registry to overwrite it with the correct sender
impl<C: Channel> Default for ChannelSender<C> {
    fn default() -> Self {
        Self {
            sender: ChannelSenderEnum::UnorderedUnreliable(UnorderedUnreliableSender::new(Duration::default())),
            channel_id: ChannelId::default(),
            writer: Writer::default(),
            marker: std::marker::PhantomData,
        }
    }
}


/// Holds information about all the channels present on the entity.
#[derive(Component, Default)]
pub struct Transport {
    // TODO: do we want to associate a Direction with the Transport?
    //  then we could only go through messages with the correct direction?
    //  Also then we would only add the receiver/sender that we need!
    //  This should be independent from server or client, so it should
    pub receivers: HashMap<ChannelId, ChannelReceiverEnum>,
    pub(crate) senders: HashMap<ChannelKind, SenderMetadata>,
    /// PriorityManager shared between all channels of this transport
    pub(crate) priority_manager: PriorityManager,
    /// PacketBuilder shared between all channels of this transport
    pub(crate) packet_manager: PacketBuilder,
    /// Map to keep track of which messages have been sent in which packets, so that
    /// reliable senders can stop trying to send a message that has already been received
    pub(crate) packet_to_message_ack_map: HashMap<PacketId, Vec<(ChannelKind, MessageAck)>>,

    pub send_mapper: SendEntityMap,
    /// Buffer to store payloads what have been processed by the transport
    pub send: Vec<SendPayload>,
    /// Buffer to store payloads that will be processed by the transport and stored in the ChannelReceiverEnum
    pub recv: Vec<RecvPayload>,
}

type FlushMessagesFn = fn(
    &mut PriorityManager,
    MutUntyped,
);

impl Transport {
    pub fn add_sender<C: Channel>(&mut self, sender_id: ComponentId, mode: ChannelMode) {
        self.senders.insert(
            ChannelKind::of::<C>(),
            SenderMetadata {
                sender_id,
                mode,
                name: core::any::type_name::<C>(),
                flush: SenderMetadata::flush_packets::<C>,
                receive_ack: SenderMetadata::receive_ack::<C>,
            },
        );
    }
}



pub(crate) struct SenderMetadata {
    /// The component id of the ChannelSender<C> component
    pub(crate) sender_id: ComponentId,
    pub(crate) mode: ChannelMode,
    pub(crate) name: &'static str,
    pub(crate) flush: FlushMessagesFn,
    pub(crate) receive_ack: fn(MutUntyped, MessageAck),
}

impl SenderMetadata {
    /// Flush packets from the ChannelSender<C> component to the actual
    /// ChannelSenderEnum
    fn flush_packets<C: Channel>(
        // TODO: ideally we would pass Transport here but split borrow issues
        priority_manager: &mut PriorityManager,
        sender: MutUntyped,
    ) {
        let mut sender = unsafe { sender.with_type::<ChannelSender<C>>() };
        let (single_data, fragment_data) = sender.sender.send_packet();
        if !single_data.is_empty() || !fragment_data.is_empty() {
            trace!(?sender.channel_id, "send message with channel_id");
            priority_manager.buffer_messages(sender.channel_id, single_data, fragment_data);
        }
    }

    fn receive_ack<C: Channel>(sender: MutUntyped, message_ack: MessageAck) {
        let mut sender = unsafe { sender.with_type::<ChannelSender<C>>() };
        sender.sender.receive_ack(&message_ack);
    }
}


impl<C: Channel> ChannelSender<C> {
    /// Buffer a message to be sent on this channel
    pub fn buffer_with_priority(
        &mut self,
        message: Bytes,
        priority: f32,
    ) -> Result<Option<MessageId>, SerializationError> {
        self.sender.buffer_send(message, priority)
    }

    /// Buffer a message to be sent on this channel
    pub fn buffer(
        &mut self,
        message: Bytes,
    ) -> Result<Option<MessageId>, SerializationError> {
        self.sender.buffer_send(message, DEFAULT_MESSAGE_PRIORITY)
    }
}



fn on_add<C: Channel>(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    let mut registry = world.resource_mut::<ChannelRegistry>();
    // TODO: merge settings and SenderId in a SenderMetadata so that we don't fetch twice
    let Some(settings) = registry.settings::<C>() else {
        panic!("ChannelSettings not found for channel {}", core::any::type_name::<C>());
    };
    let sender_id = registry.get_sender_id::<C>().unwrap();
    let channel_id = *registry.get_net_from_kind(&ChannelKind::of::<C>()).unwrap();
    let receiver: ChannelReceiverEnum;
    let sender: ChannelSenderEnum;
    let mode = settings.mode;
    match settings.mode {
        ChannelMode::UnorderedUnreliableWithAcks => {
            receiver = UnorderedUnreliableReceiver::new().into();
            sender = UnorderedUnreliableWithAcksSender::new(settings.send_frequency).into();
        }
        ChannelMode::UnorderedUnreliable => {
            receiver = UnorderedUnreliableReceiver::new().into();
            sender = UnorderedUnreliableSender::new(settings.send_frequency).into();
        }
        ChannelMode::SequencedUnreliable => {
            receiver = SequencedUnreliableReceiver::new().into();
            sender = SequencedUnreliableSender::new(settings.send_frequency).into();
        }
        ChannelMode::UnorderedReliable(reliable_settings) => {
            receiver = UnorderedReliableReceiver::new().into();
            sender = ReliableSender::new(reliable_settings, settings.send_frequency).into();
        }
        ChannelMode::SequencedReliable(reliable_settings) => {
            receiver = SequencedReliableReceiver::new().into();
            sender = ReliableSender::new(reliable_settings, settings.send_frequency).into();
        }
        ChannelMode::OrderedReliable(reliable_settings) => {
            receiver = OrderedReliableReceiver::new().into();
            sender = ReliableSender::new(reliable_settings, settings.send_frequency).into();
        }
    }
    let mut entity_mut = world.entity_mut(entity);
    let mut channel_sender = entity_mut.get_mut::<ChannelSender<C>>().unwrap();
    channel_sender.sender = sender;
    channel_sender.channel_id = channel_id;
    let mut transport = entity_mut.get_mut::<Transport>().unwrap();
    transport.add_sender::<C>(sender_id, mode);
    transport.receivers.insert(channel_id, receiver);
}


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

#[derive(Clone, Copy, PartialEq, Debug)]
/// [`ChannelDirection`] specifies in which direction the packets can be sent
pub enum ChannelDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
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

/// Default channel to replicate entity actions.
/// This is an Unordered Reliable channel.
/// (SpawnEntity, DespawnEntity, InsertComponent, RemoveComponent)
#[derive(ChannelInternal)]
pub struct EntityActionsChannel;

/// Default channel to replicate entity updates (ComponentUpdate)
/// This is a Sequenced Unreliable channel
#[derive(ChannelInternal)]
pub struct EntityUpdatesChannel;

/// Default channel to send pings. This is a Sequenced Unreliable channel, because
/// there is no point in getting older pings.
#[derive(ChannelInternal)]
pub struct PingChannel;

/// Default channel to send pongs. This is a Sequenced Unreliable channel, because
/// there is no point in getting older pongs.
#[derive(ChannelInternal)]
pub struct PongChannel;

/// Default channel to send inputs from client to server. This is a Sequenced Unreliable channel.
#[derive(ChannelInternal)]
pub struct InputChannel;

/// Channel to send messages related to Authority transfers
/// This is an Ordered Reliable channel
#[derive(ChannelInternal)]
pub struct AuthorityChannel;
