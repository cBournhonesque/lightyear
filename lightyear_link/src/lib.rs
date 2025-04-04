/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::collections::vec_deque::Drain;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::prelude::Event;
use alloc::collections::VecDeque;
use bevy::prelude::{Component, SystemSet};
use bytes::Bytes;
use core::net::SocketAddr;
use core::time::Duration;

pub mod prelude {
    pub use crate::{Link, LinkSet, LinkStats};
}

pub type RecvPayload = Bytes;
pub type SendPayload = Bytes;


/// Represents a link between two peers, allowing for sending and receiving data.
/// This only stores the payloads to be sent and received, the actual bytes will be sent by an Io component
#[derive(Component, Default)]
pub struct Link {
    // TODO: instead of Vec should we use Channels to allow parallel processing?
    //  or maybe ArrayQueue?
    /// Payloads to be received
    pub recv: LinkReceiver,
    /// Payloads to be sent
    pub send: LinkSender,

    pub stats: LinkStats,
    // TODO: maybe put this somewhere else? So that link is completely independent of how io
    //   is handled? (i.e. it might not even required a SocketAddr)
    /// Address of the remote peer
    pub remote_addr: Option<SocketAddr>,
}

impl Link {
    /// Creates a new Link with the given remote address.
    pub fn new(remote_addr: SocketAddr) -> Self {
        Self {
            recv: LinkReceiver::default(),
            send: LinkSender::default(),
            stats: LinkStats::default(),
            remote_addr: Some(remote_addr),
        }
    }
}

#[derive(Default)]
pub struct LinkReceiver(VecDeque<RecvPayload>);

impl LinkReceiver {

    pub fn drain(&mut self) -> Drain<RecvPayload> {
        self.0.drain(..)
    }

    pub fn pop(&mut self) -> Option<RecvPayload> {
        self.0.pop_front()
    }

    pub fn push(&mut self, value: RecvPayload) {
        self.0.push_back(value)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

}
#[derive(Default)]
pub struct LinkSender(VecDeque<SendPayload>);

  impl LinkSender {

    pub fn drain(&mut self) -> Drain<SendPayload> {
        self.0.drain(..)
    }

    pub fn pop(&mut self) -> Option<SendPayload> {
        self.0.pop_front()
    }

    pub fn push(&mut self, value: SendPayload) {
        self.0.push_back(value)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Link {
    pub fn send(&mut self, payload: SendPayload) {
        // TODO: stats, etc.
        self.send.push(payload);
    }
}

#[derive(Default)]
pub struct LinkStats {
    /// Number of bytes received
    pub recv_bytes: usize,
    /// Number of bytes sent
    pub send_bytes: usize,
    /// Number of packets received
    pub recv_packets: usize,
    /// Number of packets sent
    pub send_packets: usize,
    pub rtt: Duration,
    pub jitter: Duration,
}


// TODO: add things here that are entirely dependent on the link
//  - packet lost stats?
//  - rtt/jitter estimate

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum LinkSet {
    // PRE UPDATE
    /// Receive bytes from the IO and buffer them into the Link
    Receive,

    // PostUpdate
    /// Flush the messages buffered in the Link to the io
    Send,
}

#[derive(Event)]
pub struct LinkStart;

#[derive(Event)]
pub struct Unlink;

#[derive(Component, Default, Debug)]
pub struct Linking;

#[derive(Component, Default, Debug)]
pub struct Linked;

#[derive(Component, Default, Debug)]
pub struct Unlinked;
