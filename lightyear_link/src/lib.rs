//! # Lightyear Link
//!
//! This crate defines the `Link` component and related abstractions for network communication.
//! A `Link` represents a connection to a remote peer and handles the buffering of incoming
//! and outgoing byte payloads. It is transport-agnostic, meaning the actual sending and
//! receiving of bytes over a physical (or virtual) network is handled by a separate IO layer
//! (e.g., UDP, WebTransport, Crossbeam channels).
//!
//! It also includes features like:
//! - Link conditioning (simulating latency, jitter, packet loss) via `LinkConditioner`.
//! - Link state management (`Linked`, `Linking`, `Unlinked`).
//! - Basic link statistics (`LinkStats`).
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod conditioner;
mod id;
pub mod server;

use alloc::{collections::vec_deque::Drain, string::String};

pub use crate::conditioner::LinkConditioner;
use alloc::collections::VecDeque;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::{
    component::{Component, HookContext},
    event::Event,
    observer::Trigger,
    schedule::{IntoScheduleConfigs, SystemSet},
    system::{Commands, Query},
    world::DeferredWorld,
};
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::time::Instant;

/// Commonly used items from the `lightyear_link` crate.
pub mod prelude {
    pub use crate::conditioner::LinkConditionerConfig;
    pub use crate::server::{LinkOf, Server};
    pub use crate::{
        Link, LinkSet, LinkStart, LinkStats, Linked, Linking, RecvLinkConditioner, Unlink, Unlinked,
    };

    pub mod server {
        pub use crate::server::{LinkOf, Server};
    }
}

pub type RecvPayload = Bytes;
pub type SendPayload = Bytes;

/// Represents the current connection state of a `Link`.
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
pub struct Link {
    // TODO: instead of Vec should we use Channels to allow parallel processing?
    //  or maybe ArrayQueue?
    /// Payloads to be received
    pub recv: LinkReceiver,
    /// Payloads to be sent
    pub send: LinkSender,
    pub state: LinkState,
    pub stats: LinkStats,
}

/// Type alias for a `LinkConditioner` specifically for receiving `RecvPayload`.
///
/// This is used to simulate network conditions (latency, jitter, packet loss)
/// on incoming packets.
pub type RecvLinkConditioner = LinkConditioner<RecvPayload>;

impl Link {
    /// Creates a new Link with the given remote address.
    pub fn new(recv_conditioner: Option<RecvLinkConditioner>) -> Self {
        Self {
            recv: LinkReceiver {
                buffer: VecDeque::new(),
                conditioner: recv_conditioner,
            },
            send: LinkSender::default(),
            state: Default::default(),
            stats: LinkStats::default(),
        }
    }
}

/// Handles receiving and buffering incoming payloads for a `Link`.
///
/// It contains a buffer for payloads and an optional `LinkConditioner`
/// to simulate network conditions on received data.
#[derive(Default)]
pub struct LinkReceiver {
    buffer: VecDeque<RecvPayload>,
    pub conditioner: Option<LinkConditioner<RecvPayload>>,
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

    pub fn push(&mut self, value: RecvPayload, instant: Instant) {
        if let Some(conditioner) = &mut self.conditioner {
            conditioner.condition_packet(value, instant);
        } else {
            self.push_raw(value);
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    #[cfg(feature = "test_utils")]
    pub fn iter(&self) -> impl Iterator<Item = &SendPayload> {
        self.buffer.iter()
    }
}
/// Handles buffering outgoing payloads for a `Link`.
///
/// It contains a buffer for payloads that are ready to be sent by the
/// underlying IO transport.
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

    #[cfg(feature = "test_utils")]
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

/// Stores statistics about a `Link`, such as bytes/packets sent and received, RTT, and jitter.
#[derive(Default)]
pub struct LinkStats {
    pub rtt: Duration,
    pub jitter: Duration,
}

// TODO: add things here that are entirely dependent on the link
//  - packet lost stats?
//  - rtt/jitter estimate

/// System sets for `Link`-related operations.
///
/// These are used to order systems that handle:
/// - Receiving data from the IO layer into the `Link` buffer.
/// - Applying link conditioning to received packets.
/// - Sending data from the `Link` buffer to the IO layer.
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

/// Event triggered to initiate the process of establishing a `Link`.
///
/// This event typically signals the underlying IO layer to start connecting
/// to a remote peer.
#[derive(Event)]
pub struct LinkStart;

/// Event triggered to initiate the process of terminating a `Link`.
///
/// This event typically signals the underlying IO layer to disconnect
/// from the remote peer.
#[derive(Event, Clone, Debug)]
pub struct Unlink {
    pub reason: String,
}

#[derive(Component, Default, Debug)]
#[component(on_insert = Linking::on_insert)]
pub struct Linking;

impl Linking {
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut link) = world.get_mut::<Link>(context.entity) {
            link.state = LinkState::Linking;
        }
        world
            .commands()
            .entity(context.entity)
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
        world
            .commands()
            .entity(context.entity)
            .remove::<(Linking, Unlinked)>();
    }
}

#[derive(Component, Default, Debug)]
#[component(on_insert = Unlinked::on_insert)]
pub struct Unlinked {
    pub reason: String,
}

impl Unlinked {
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        if let Some(mut link) = world.get_mut::<Link>(context.entity) {
            link.state = LinkState::Unlinked;
        }
        world
            .commands()
            .entity(context.entity)
            .remove::<(Linked, Linking)>();
    }
}

/// Bevy plugin that sets up the systems for managing `Link` components.
///
/// This plugin configures system sets for:
/// - Receiving data into `Link` buffers (`LinkSet::Receive`).
/// - Applying link conditioning (`LinkSet::ApplyConditioner`).
/// - Sending data from `Link` buffers (`LinkSet::Send`).
///
/// It also includes a system to apply the `LinkConditioner` if present on a `Link`.
pub struct LinkPlugin;

impl LinkPlugin {
    pub fn apply_link_conditioner(mut query: Query<&mut Link>) {
        query.par_iter_mut().for_each(|mut link| {
            // enable split borrows
            let recv = &mut link.recv;
            if let Some(conditioner) = &mut recv.conditioner {
                while let Some(packet) = conditioner.pop_packet(Instant::now()) {
                    // cannot use push_raw() because of partial borrows issue
                    recv.buffer.push_back(packet);
                }
            }
        });
    }

    /// If the user requested to unlink, then we insert the Unlinked component
    fn unlink(mut trigger: Trigger<Unlink>, mut commands: Commands) {
        if let Ok(mut c) = commands.get_entity(trigger.target()) {
            c.insert(Unlinked {
                reason: core::mem::take(&mut trigger.reason),
            });
        }
    }
}

impl Plugin for LinkPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            Self::apply_link_conditioner.in_set(LinkSet::ApplyConditioner),
        );
        app.configure_sets(
            PreUpdate,
            (LinkSet::Receive, LinkSet::ApplyConditioner).chain(),
        );
        app.configure_sets(PostUpdate, LinkSet::Send);

        app.add_observer(Self::unlink);
    }
}
