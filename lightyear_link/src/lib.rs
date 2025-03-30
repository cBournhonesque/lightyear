/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::prelude::{Component, SystemSet};
use bytes::Bytes;
use core::net::SocketAddr;

pub mod ping;

pub type RecvPayload = Bytes;
pub type SendPayload = Bytes;


// We will have one component Io<Type> per actual io (webtransport, UDP, etc.)

/// Represents a link between two peers, allowing for sending and receiving data.
/// This only stores the payloads to be sent and received, the actual bytes will be sent by an Io component
#[derive(Component, Default)]
pub struct Link {
    /// Payloads to be received
    pub recv: Vec<RecvPayload>,
    /// Payloads to be sent
    pub send: Vec<SendPayload>,

    // TODO: maybe put this somewhere else? So that link is completely independent of how io
    //   is handled? (i.e. it might not even required a SocketAddr)
    /// Address of the remote peer
    pub remote_addr: Option<SocketAddr>,
}

impl Link {
    /// Creates a new Link with the given remote address.
    pub fn new(remote_addr: SocketAddr) -> Self {
        Self {
            recv: Vec::new(),
            send: Vec::new(),
            remote_addr: Some(remote_addr),
        }
    }
}

pub type LinkReceiver = Vec<RecvPayload>;
pub type LinkSender = Vec<SendPayload>;

impl Link {
    pub fn send(&mut self, payload: SendPayload) {
        // TODO: stats, etc.
        self.send.push(payload);
    }
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