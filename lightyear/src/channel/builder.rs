//! This module contains the [`Channel`] trait
use lightyear_macros::ChannelInternal;
use std::time::Duration;

use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
use crate::channel::receivers::sequenced_reliable::SequencedReliableReceiver;
use crate::channel::receivers::sequenced_unreliable::SequencedUnreliableReceiver;
use crate::channel::receivers::tick_unreliable::TickUnreliableReceiver;
use crate::channel::receivers::unordered_reliable::UnorderedReliableReceiver;
use crate::channel::receivers::unordered_unreliable::UnorderedUnreliableReceiver;
use crate::channel::receivers::ChannelReceiver;
use crate::channel::senders::reliable::ReliableSender;
use crate::channel::senders::sequenced_unreliable::SequencedUnreliableSender;
use crate::channel::senders::tick_unreliable::TickUnreliableSender;
use crate::channel::senders::unordered_unreliable::UnorderedUnreliableSender;
use crate::channel::senders::unordered_unreliable_with_acks::UnorderedUnreliableWithAcksSender;
use crate::channel::senders::ChannelSender;
use crate::utils::named::TypeNamed;

/// A ChannelContainer is a struct that implements the [`Channel`] trait
pub struct ChannelContainer {
    pub setting: ChannelSettings,
    pub(crate) receiver: ChannelReceiver,
    pub(crate) sender: ChannelSender,
}

/// A Channel is an abstraction for a way to send messages over the network
/// You can define the direction, ordering, reliability of the channel
pub trait Channel: 'static + TypeNamed {
    fn get_builder(settings: ChannelSettings) -> ChannelBuilder;
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct ChannelBuilder {
    // TODO: this has been made public just for testing integration tests
    pub settings: ChannelSettings,
}

impl ChannelBuilder {
    pub fn build(&self) -> ChannelContainer {
        ChannelContainer::new(self.settings.clone())
    }
}

impl ChannelContainer {
    pub fn new(settings: ChannelSettings) -> Self {
        let receiver: ChannelReceiver;
        let sender: ChannelSender;
        let settings_clone = settings.clone();
        match settings.mode {
            ChannelMode::UnorderedUnreliableWithAcks => {
                receiver = UnorderedUnreliableReceiver::new().into();
                sender = UnorderedUnreliableWithAcksSender::new().into();
            }
            ChannelMode::UnorderedUnreliable => {
                receiver = UnorderedUnreliableReceiver::new().into();
                sender = UnorderedUnreliableSender::new().into();
            }
            ChannelMode::SequencedUnreliable => {
                receiver = SequencedUnreliableReceiver::new().into();
                sender = SequencedUnreliableSender::new().into();
            }
            ChannelMode::UnorderedReliable(reliable_settings) => {
                receiver = UnorderedReliableReceiver::new().into();
                sender = ReliableSender::new(reliable_settings).into();
            }
            ChannelMode::SequencedReliable(reliable_settings) => {
                receiver = SequencedReliableReceiver::new().into();
                sender = ReliableSender::new(reliable_settings).into();
            }
            ChannelMode::OrderedReliable(reliable_settings) => {
                receiver = OrderedReliableReceiver::new().into();
                sender = ReliableSender::new(reliable_settings).into();
            }
            ChannelMode::TickBuffered => {
                receiver = TickUnreliableReceiver::new().into();
                sender = TickUnreliableSender::new().into();
            }
        }
        Self {
            setting: settings_clone,
            receiver,
            sender,
        }
    }
}

/// [`ChannelSettings`] are used to specify how the [`Channel`] behaves (reliability, ordering, direction)
#[derive(Clone, Debug)]
pub struct ChannelSettings {
    // TODO: split into Ordering and Reliability? Or not because we might to add new modes like TickBuffered
    pub mode: ChannelMode,
    pub direction: ChannelDirection,
}

#[derive(Clone, Debug, PartialEq)]
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
    /// Inputs from the client are associated with the current tick on the client.
    /// The server will buffer them and only receive them on the same tick.
    TickBuffered,
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
            ChannelMode::TickBuffered => false,
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
            ChannelMode::TickBuffered => false,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
/// [`ChannelDirection`] specifies in which direction the packets can be sent
pub enum ChannelDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReliableSettings {
    /// Duration to wait before resending a packet if it has not been acked
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
        std::cmp::max(delay, self.rtt_resend_min_delay)
    }
}

/// Default channel to replicate entity actions.
/// This is an Unordered Reliable channel.
/// (SpawnEntity, DespawnEntity, InsertComponent, RemoveComponent)
#[derive(ChannelInternal)]
pub struct EntityActionsChannel;

#[derive(ChannelInternal)]
/// Default channel to replicate entity updates (ComponentUpdate)
/// This is a Sequenced Unreliable channel
pub struct EntityUpdatesChannel;

/// Default channel to send pings. This is a Sequenced Unreliable channel, because
/// there is no point in getting older pings.
#[derive(ChannelInternal)]
pub struct PingChannel;

// TODO: should we use sequenced or unordered?
#[derive(ChannelInternal)]
/// Default channel to send inputs from client to server. This is a Sequenced Unreliable channel.
pub struct InputChannel;

/// Default Unordedered Unreliable channel, to send messages as fast as possible without any ordering.
#[derive(ChannelInternal)]
pub struct DefaultUnorderedUnreliableChannel;

/// Channel where the messages are buffered according to the tick they are associated with
/// At each server tick, we can read the messages that were sent from the corresponding client tick
#[derive(ChannelInternal)]
pub struct TickBufferChannel;
