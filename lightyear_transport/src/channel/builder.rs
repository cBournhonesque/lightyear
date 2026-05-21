//! Channel settings and delivery modes.
//!
//! [`ChannelSettings`](crate::channel::builder::ChannelSettings) describes the delivery mode, send
//! cadence, and priority for a registered channel type.
use core::time::Duration;

#[doc(hidden)]
pub use crate::transport::{ReceiverMetadata, SenderMetadata, Transport};

/// Default priority applied to messages when no explicit message priority is provided.
pub const DEFAULT_MESSAGE_PRIORITY: f32 = 1.0;

/// Delivery and scheduling settings for a registered [`Channel`](crate::channel::Channel).
///
/// These settings are stored in [`ChannelRegistry`](crate::channel::registry::ChannelRegistry) and
/// used to construct the matching sender and receiver state for each [`Transport`] entity.
/// Direction is not stored here; it is configured via
/// [`ChannelRegistration::add_direction`](crate::channel::registry::ChannelRegistration::add_direction).
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
    /// See [`PriorityManager`](crate::packet::priority_manager::PriorityManager) for more
    /// information.
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

/// Delivery semantics for a channel.
///
/// These modes are inspired by RakNet-style reliability types. They control whether messages are
/// acknowledged/retried, whether old sequenced messages are dropped, and whether receive order is
/// preserved.
///
/// See <http://www.jenkinssoftware.com/raknet/manual/reliabilitytypes.html> for the terminology
/// this API follows.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Returns `true` if messages in this mode are retried until acknowledged.
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

/// Resend tuning for reliable channel modes.
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

/// Default channel for client-to-server input messages.
///
/// Applications normally register this as a sequenced unreliable channel so only the newest input
/// for a tick stream matters.
pub struct InputChannel;

/// Default channel for authority-transfer messages.
///
/// Applications normally register this as ordered reliable because authority transitions must arrive
/// and be processed in order.
pub struct AuthorityChannel;
