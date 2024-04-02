//! Module defining 'wrappers' that modify the behaviour of an existing [`PacketReceiver`] or [`PacketSender`].
//!
//! Wrappers are used to add additional functionality to an existing transport, such as encryption, compression, metrics, etc.
use crate::transport::{PacketReceiver, PacketSender};

/// A conditioner is used to simulate network conditions such as latency, jitter and packet loss.
pub(crate) mod conditioner;

pub trait PacketReceiverWrapper {
    fn wrap(&mut self, receiver: Box<&mut dyn PacketReceiver>) -> Box<&mut dyn PacketReceiver>;
}

pub trait PacketSenderWrapper {
    fn wrap(&mut self, sender: Box<&mut dyn PacketSender>) -> Box<&mut dyn PacketSender>;
}
