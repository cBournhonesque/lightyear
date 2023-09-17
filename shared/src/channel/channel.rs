use crate::channel::channel::ChannelMode::{SequencedReliable, SequencedUnreliable};
use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
use crate::channel::receivers::sequenced_reliable::SequencedReliableReceiver;
use crate::channel::receivers::unordered_reliable::UnorderedReliableReceiver;
use crate::channel::receivers::unordered_unreliable::UnorderedUnreliableReceiver;
use crate::channel::receivers::ChannelReceiver;
use crate::channel::senders::reliable::ReliableSender;
use crate::channel::senders::unreliable::UnorderedUnreliableSender;
use crate::channel::senders::ChannelSender;
use serde::Serialize;
use std::any::TypeId;

/// A Channel is an abstraction for a way to send messages over the network
/// You can defined the direction, ordering, reliability of the channel
pub struct Channel {
    setting: ChannelSettings,
    kind: ChannelKind,
    pub(crate) receiver: Box<dyn ChannelReceiver>,
    pub(crate) sender: Box<dyn ChannelSender>,
}

/// Data from the channel that will be serialized in the header of the packet
pub(crate) struct ChannelHeader {
    pub(crate) kind: ChannelKind,
    // TODO: add fragmentation data
}

impl Channel {
    pub fn new(settings: &ChannelSettings, kind: ChannelKind) -> Self {
        let mut channel = Self {
            setting: settings.clone(),
            kind,
            receiver: Box::new(()),
            sender: Box::new(()),
        };
        match &settings.mode {
            ChannelMode::UnorderedUnreliable => {
                channel.receiver = Box::new(UnorderedUnreliableReceiver::new());
                channel.sender = Box::new(UnorderedUnreliableSender::new());
            }
            ChannelMode::SequencedUnreliable => {
                // TODO:
                // self.receiver = Box::new(SequencedUnreliableReceiver::new());
                // self.sender = Box::new(UnorderedUnreliableSender::new());
            }
            ChannelMode::UnorderedReliable(reliable_settings) => {
                channel.receiver = Box::new(ReliableSender::new());
                channel.sender = Box::new(SequencedReliableReceiver::new());
            }
            ChannelMode::SequencedReliable(reliable_settings) => {
                channel.receiver = Box::new(SequencedReliableReceiver::new());
            }
            ChannelMode::OrderedReliable(reliable_settings) => {
                channel.receiver = Box::new(OrderedReliableReceiver::new());
                channel.sender = Box::new(ReliableSender::new());
            }
        }
        channel
    }
}

/// Type of the channel
// TODO: update the serialization
#[derive(Serialize)]
pub struct ChannelKind(TypeId);

#[derive(Clone)]
pub struct ChannelSettings {
    pub mode: ChannelMode,
    pub direction: ChannelDirection,
}

pub enum ChannelOrdering {
    /// Messages will arrive in the order that they were sent
    Ordered,
    /// Messages will arrive in any order
    Unordered,
    /// Only the newest messages are accepted; older messages are discarded
    Sequenced,
}

#[derive(Clone)]
/// ChannelMode specifies how packets are sent and received
/// See more information: http://www.jenkinssoftware.com/raknet/manual/reliabilitytypes.html
pub enum ChannelMode {
    /// Packets may arrive out-of-order, or not at all
    UnorderedUnreliable,
    /// Same as unordered unreliable, but only the newest packet is ever accepted, older packets
    /// are ignored
    SequencedUnreliable,
    /// Packets may arrive out-of-order, but we make sure (with retries, acks) that the packet
    /// will arrive
    UnorderedReliable(ReliableSettings),
    /// Same as unordered reliable, but the packets are sequenced (only the newest packet is accepted)
    SequencedReliable(ReliableSettings),
    /// Packets will arrive in the correct order at the destination
    OrderedReliable(ReliableSettings),
}

#[derive(Clone, Eq, PartialEq)]
pub enum ChannelDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}

#[derive(Clone)]
pub struct ReliableSettings {
    /// TODO
    pub rtt_resend_factor: f32,
}

impl ReliableSettings {
    pub const fn default() -> Self {
        Self {
            rtt_resend_factor: 1.5,
        }
    }
}
