/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
mod conditioner;
mod server;
mod id;

use alloc::collections::vec_deque::Drain;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::conditioner::LinkConditioner;
use alloc::collections::VecDeque;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bytes::Bytes;
use core::net::SocketAddr;
use core::time::Duration;

pub mod prelude {
    pub use crate::server::{LinkOf, ServerLink};
    pub use crate::{Link, LinkSet, LinkStart, LinkStats, Linking, RecvLinkConditioner, Unlinked};
}

pub type RecvPayload = Bytes;
pub type SendPayload = Bytes;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// The link is not established
    Linked,
    /// The link is in process
    Linking,
    #[default]
    /// The entity is not linked to the remote entity
    Unlinked,
}


/// Represents a link between two peers, allowing for sending and receiving data.
/// This only stores the payloads to be sent and received, the actual bytes will be sent by an Io component
#[derive(Component, Default)]
#[require(Unlinked)]
pub struct Link {
    // TODO: instead of Vec should we use Channels to allow parallel processing?
    //  or maybe ArrayQueue?
    /// Payloads to be received
    pub recv: LinkReceiver,
    /// Payloads to be sent
    pub send: LinkSender,

    pub state: LinkState,
    pub stats: LinkStats,
    // TODO: maybe put this somewhere else? So that link is completely independent of how io
    //   is handled? (i.e. it might not even required a SocketAddr)
    //   maybe we have a LinkId, and for example netcode would only be compatible if the LinkId has a SocketAddr?
    /// Address of the remote peer
    pub remote_addr: Option<SocketAddr>,
}

pub type RecvLinkConditioner = LinkConditioner<RecvPayload>;

impl Link {
    /// Creates a new Link with the given remote address.
    pub fn new(remote_addr: SocketAddr, recv_conditioner: Option<RecvLinkConditioner>) -> Self {
        Self {
            recv: LinkReceiver {
                buffer: VecDeque::new(),
                conditioner: recv_conditioner,
            },
            send: LinkSender::default(),
            state: Default::default(),
            stats: LinkStats::default(),
            remote_addr: Some(remote_addr),
        }
    }
}

#[derive(Default)]
pub struct LinkReceiver{
    buffer: VecDeque<RecvPayload>,
    conditioner: Option<LinkConditioner<RecvPayload>>,
}

impl LinkReceiver {

    pub fn drain(&mut self) -> Drain<RecvPayload> {
        self.buffer.drain(..)
    }

    pub fn pop(&mut self) -> Option<RecvPayload> {
        self.buffer.pop_front()
    }

    /// Push the payload directly to the buffer with no conditioning
    pub fn push_raw(&mut self, value: RecvPayload) {
        self.buffer.push_back(value);
    }

    pub fn push(&mut self, value: RecvPayload, elapsed: Duration) {
        if let Some(conditioner) = &mut self.conditioner {
            conditioner.condition_packet(value, elapsed);
        } else {
            self.push_raw(value);
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    #[cfg(any(test, feature = "test_utils"))]
    pub fn iter(&self) -> impl Iterator<Item = &SendPayload> {
         self.buffer.iter()
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

    #[cfg(any(test, feature = "test_utils"))]
    pub fn iter(&self) -> impl Iterator<Item = &SendPayload> {
        self.0.iter()
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
    /// Apply Link Conditioner on the receive side
    ApplyConditioner,

    // PostUpdate
    /// Flush the messages buffered in the Link to the io
    Send,
}

#[derive(Event)]
pub struct LinkStart;

#[derive(Event)]
pub struct Unlink;

#[derive(Component, Default, Debug)]
#[component(on_insert = Linking::on_insert)]
pub struct Linking;

impl Linking {
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut link) = world.get_mut::<Link>(context.entity) {
            link.state = LinkState::Linking;
        }
        world.commands().entity(context.entity)
            .remove::<(Linked, Unlinked)>();
    }
}

#[derive(Component, Default, Debug)]
#[component(on_insert = Linked::on_insert)]
pub struct Linked;

impl Linked {
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut link) = world.get_mut::<Link>(context.entity) {
            link.state = LinkState::Linked;
        }
        world.commands().entity(context.entity)
            .remove::<(Linking, Unlinked)>();
    }
}

#[derive(Component, Default, Debug)]
#[component(on_insert = Unlinked::on_insert)]
pub struct Unlinked {
    pub reason: Option<String>,
}

impl Unlinked {
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut link) = world.get_mut::<Link>(context.entity) {
            link.state = LinkState::Unlinked;
        }
        world.commands().entity(context.entity)
            .remove::<(Linked, Linking)>();
    }
}


pub struct LinkPlugin;

impl LinkPlugin {
    pub fn apply_link_conditioner(
        time: Res<Time<Real>>,
        mut query: Query<&mut Link>,
    ) {
        query.par_iter_mut().for_each(|mut link| {
            // enable split borrows
            let recv = &mut link.recv;
            if let Some(conditioner) = &mut recv.conditioner {
                while let Some(packet) = conditioner.pop_packet(time.elapsed()) {
                    // cannot use push_raw() because of partial borrows issue
                    recv.buffer.push_back(packet);
                }
            }
        });
    }
}

impl Plugin for LinkPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(PreUpdate, (LinkSet::Receive, LinkSet::ApplyConditioner).chain());
        app.configure_sets(PostUpdate, LinkSet::Send);
    }
}
