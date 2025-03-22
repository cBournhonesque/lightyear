//! This module contains the [`Channel`] trait
use core::time::Duration;

use lightyear_macros::ChannelInternal;

use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
use crate::channel::receivers::sequenced_reliable::SequencedReliableReceiver;
use crate::channel::receivers::sequenced_unreliable::SequencedUnreliableReceiver;
use crate::channel::receivers::unordered_reliable::UnorderedReliableReceiver;
use crate::channel::receivers::unordered_unreliable::UnorderedUnreliableReceiver;
use crate::channel::receivers::ChannelReceiver;
use crate::channel::senders::reliable::ReliableSender;
use crate::channel::senders::sequenced_unreliable::SequencedUnreliableSender;
use crate::channel::senders::unordered_unreliable::UnorderedUnreliableSender;
use crate::channel::senders::unordered_unreliable_with_acks::UnorderedUnreliableWithAcksSender;
use crate::channel::senders::ChannelSender;
#[cfg(feature = "trace")]
use crate::channel::stats::send::ChannelSendStats;
use crate::prelude::ChannelKind;

/// A ChannelContainer is a struct that implements the [`Channel`] trait
#[derive(Debug)]
pub struct ChannelContainer {
    pub name: &'static str,
    pub setting: ChannelSettings,
    pub(crate) receiver: ChannelReceiver,
    pub(crate) sender: ChannelSender,
    // we will put this behind the trace feature for now, as this is pretty niche
    // and might be performance heavy
    #[cfg(feature = "trace")]
    pub(crate) sender_stats: ChannelSendStats,
}

/// A `Channel` is an abstraction for a way to send messages over the network
/// You can define the direction, ordering, reliability of the channel.
///
/// # Example
///
/// Here is how you can add a new channel to the protocol. Messages sent on this channel will be unordered;
/// they can be lost (no reliability guarantee) and they can be sent in both directions.
///
/// ```rust,ignore
/// #[derive(Channel)]
/// struct MyChannel;
///
/// app.add_channel::<MyChannel>(ChannelSettings {
///     mode: ChannelMode::UnorderedUnreliable,
///     direction: ChannelDirection::Bidirectional,
///     priority: 1.0,
/// });
/// ```
pub trait Channel: 'static {
    fn get_builder(settings: ChannelSettings) -> ChannelBuilder {
        ChannelBuilder {
            name: Self::name(),
            settings,
        }
    }

    fn name() -> &'static str;

    fn kind() -> ChannelKind
    where
        Self: Sized,
    {
        ChannelKind::of::<Self>()
    }
}

#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelBuilder {
    pub name: &'static str,
    pub settings: ChannelSettings,
}

impl ChannelBuilder {
    pub fn build(&self) -> ChannelContainer {
        ChannelContainer::new(self.name, self.settings.clone())
    }
}

impl ChannelContainer {
    pub fn new(name: &'static str, settings: ChannelSettings) -> Self {
        let receiver: ChannelReceiver;
        let sender: ChannelSender;
        let settings_clone = settings.clone();
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
        Self {
            name,
            setting: settings_clone,
            receiver,
            sender,
            #[cfg(feature = "trace")]
            sender_stats: ChannelSendStats::default(),
        }
    }
}

/// [`ChannelSettings`] are used to specify how the [`Channel`] behaves (reliability, ordering, direction)
#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
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

#[derive(ChannelInternal)]
/// Default channel to replicate entity updates (ComponentUpdate)
/// This is a Sequenced Unreliable channel
pub struct EntityUpdatesChannel;

/// Default channel to send pings. This is a Sequenced Unreliable channel, because
/// there is no point in getting older pings.
#[derive(ChannelInternal)]
pub struct PingChannel;

/// Default channel to send pongs. This is a Sequenced Unreliable channel, because
/// there is no point in getting older pongs.
#[derive(ChannelInternal)]
pub struct PongChannel;

#[derive(ChannelInternal)]
/// Default channel to send inputs from client to server. This is a Sequenced Unreliable channel.
pub struct InputChannel;

#[derive(ChannelInternal)]
/// Channel to send messages related to Authority transfers
/// This is an Ordered Reliable channel
pub struct AuthorityChannel;
