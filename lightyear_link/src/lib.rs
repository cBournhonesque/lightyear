//! Transport-agnostic link buffers and link lifecycle markers.
//!
//! A [`Link`] is Lightyear's transport-neutral boundary between higher-level networking
//! systems and concrete IO backends. Protocol, connection, replication, and message systems
//! read from and write to [`Link`] buffers; transport crates such as `lightyear_udp`,
//! `lightyear_webtransport`, `lightyear_websocket`, `lightyear_steam`, and
//! `lightyear_crossbeam` are responsible for moving those [`bytes::Bytes`] payloads across an
//! actual network or in-process channel.
//!
//! The crate deliberately keeps the IO abstraction narrow:
//! - [`RecvPayload`] and [`SendPayload`] are opaque byte payloads.
//! - [`LinkReceiver`] buffers payloads received from a transport until higher-level systems
//!   consume them.
//! - [`LinkSender`] buffers payloads produced by higher-level systems until a transport flushes
//!   them.
//! - [`LinkConditioner`] can delay or drop inbound payloads to simulate imperfect networks.
//! - [`Linking`], [`Linked`], and [`Unlinked`] are mutually exclusive ECS marker components that
//!   keep [`Link::state`] synchronized with the entity lifecycle.
//!
//! Server-side fan-out relationships live in [`server`].
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod conditioner;
pub mod server;

use alloc::{collections::vec_deque::Drain, string::String};

pub use crate::conditioner::LinkConditioner;
use alloc::collections::VecDeque;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::Reflect;
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::time::Instant;

pub mod prelude {
    pub use crate::conditioner::LinkConditionerConfig;
    pub use crate::server::{LinkOf, Server};
    pub use crate::{
        Link, LinkStart, LinkStats, LinkSystems, Linked, Linking, RecvLinkConditioner, Unlink,
        UnlinkReason, Unlinked,
    };

    pub mod server {
        pub use crate::server::{LinkOf, Server};
    }
}

/// Opaque byte payload received from a transport.
///
/// A transport pushes this payload into [`LinkReceiver`] after decoding any transport-specific
/// envelope. Higher-level Lightyear systems then interpret the bytes as messages, replication
/// data, connection packets, or other protocol frames.
pub type RecvPayload = Bytes;

/// Opaque byte payload queued for a transport to send.
///
/// Higher-level Lightyear systems enqueue this payload through [`Link::send`] or [`LinkSender`].
/// A transport drains [`LinkSender`] in [`LinkSystems::Send`] and writes the bytes to its concrete
/// IO backend.
pub type SendPayload = Bytes;

/// Current lifecycle state of a [`Link`].
///
/// This enum mirrors the mutually exclusive marker components [`Linking`], [`Linked`], and
/// [`Unlinked`]. User code usually inserts the marker components rather than mutating this value
/// directly, because the marker hooks also remove the other lifecycle markers.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// The link is established and can exchange payloads.
    Linked,
    /// The link is being established by the transport.
    Linking,
    /// The link is not connected to a remote peer.
    #[default]
    Unlinked,
}

/// Transport-neutral byte stream between two peers.
///
/// `Link` is an ECS component owned by the entity that represents a local transport endpoint or a
/// remote peer. It only stores buffered payloads and lifecycle/statistics state; concrete IO is
/// handled by transport-specific components in crates such as `lightyear_udp`,
/// `lightyear_crossbeam`, `lightyear_webtransport`, or `lightyear_steam`.
///
/// Incoming bytes flow: transport -> [`LinkReceiver`] -> higher-level systems.
/// Outgoing bytes flow: higher-level systems -> [`LinkSender`] -> transport.
#[derive(Component, Default)]
pub struct Link {
    /// Payloads received from the transport and waiting to be consumed by Lightyear systems.
    pub recv: LinkReceiver,
    /// Payloads produced by Lightyear systems and waiting to be flushed by the transport.
    pub send: LinkSender,
    /// Cached lifecycle state mirrored from [`Linking`], [`Linked`], or [`Unlinked`].
    pub state: LinkState,
    /// Transport-observed statistics for this link.
    pub stats: LinkStats,
}

/// Packet conditioner used for inbound [`RecvPayload`] values.
///
/// For symmetric simulations, construct two links with matching or split
/// [`prelude::LinkConditionerConfig`] values.
pub type RecvLinkConditioner = LinkConditioner<RecvPayload>;

impl Link {
    /// Creates a link with empty send/receive buffers.
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

/// Receive-side payload queue for a [`Link`].
///
/// Transports push network payloads into this queue, and higher-level Lightyear systems drain or
/// pop them during [`LinkSystems::Receive`].
///
/// If [`conditioner`](Self::conditioner) is present,
/// [`push`](Self::push) routes packets through [`LinkConditioner`] before they become visible in
/// the buffer.
#[derive(Default)]
pub struct LinkReceiver {
    buffer: VecDeque<RecvPayload>,
    /// Optional receive-side link conditioner for latency, jitter, and packet-loss simulation.
    pub conditioner: Option<LinkConditioner<RecvPayload>>,
}

impl LinkReceiver {
    /// Drains every currently available received payload in FIFO order.
    ///
    /// Conditioned packets that are not ready yet remain in the [`LinkConditioner`] and are not
    /// yielded by this iterator.
    pub fn drain(&mut self) -> Drain<'_, RecvPayload> {
        self.buffer.drain(..)
    }

    /// Removes and returns the oldest available received payload.
    ///
    /// Returns `None` when the receive buffer is empty.
    pub fn pop(&mut self) -> Option<RecvPayload> {
        self.buffer.pop_front()
    }

    /// Appends a received payload directly to the available buffer.
    ///
    /// This bypasses [`conditioner`](Self::conditioner). Transport code should use this when it is
    /// replaying already-conditioned data, injecting test packets, or implementing an IO backend
    /// that intentionally does not simulate network conditions.
    pub fn push_raw(&mut self, value: RecvPayload) {
        self.buffer.push_back(value);
    }

    /// Appends a received payload, applying the configured link conditioner if present.
    ///
    /// `instant` is the local receive time used as the base timestamp for simulated latency and
    /// jitter. When no conditioner is configured, this is equivalent to [`push_raw`](Self::push_raw).
    pub fn push(&mut self, value: RecvPayload, instant: Instant) {
        if let Some(conditioner) = &mut self.conditioner {
            conditioner.condition_packet(value, instant);
        } else {
            self.push_raw(value);
        }
    }

    /// Returns the number of payloads currently available to consumers.
    ///
    /// Packets still delayed inside [`conditioner`](Self::conditioner) are not included.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Iterates over the currently available received payloads without consuming them.
    #[cfg(feature = "test_utils")]
    pub fn iter(&self) -> impl Iterator<Item = &SendPayload> {
        self.buffer.iter()
    }
}

/// Send-side payload queue for a [`Link`].
///
/// Higher-level systems enqueue payloads here. Transport plugins drain the queue during
/// [`LinkSystems::Send`] and write each [`SendPayload`] to their concrete IO backend.
#[derive(Default)]
pub struct LinkSender(VecDeque<SendPayload>);

impl LinkSender {
    /// Drains every queued outgoing payload in FIFO order.
    ///
    /// Transport systems typically call this in [`LinkSystems::Send`] once they are ready to flush
    /// all pending packets for the frame or tick.
    pub fn drain(&mut self) -> Drain<'_, SendPayload> {
        self.0.drain(..)
    }

    /// Removes and returns the oldest queued outgoing payload.
    ///
    /// This is useful for transports that send one packet at a time or need to requeue a packet
    /// with [`push_front`](Self::push_front) if the backend reports backpressure.
    pub fn pop(&mut self) -> Option<SendPayload> {
        self.0.pop_front()
    }

    /// Appends an outgoing payload to the back of the FIFO queue.
    pub fn push(&mut self, value: SendPayload) {
        self.0.push_back(value)
    }

    /// Prepends an outgoing payload to the front of the queue.
    pub fn push_front(&mut self, value: SendPayload) {
        self.0.push_front(value)
    }

    /// Returns the number of outgoing payloads waiting to be flushed.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterates over queued outgoing payloads without consuming them.
    #[cfg(feature = "test_utils")]
    pub fn iter(&self) -> impl Iterator<Item = &SendPayload> {
        self.0.iter()
    }
}

impl Link {
    /// Queues an outgoing payload for the transport layer.
    ///
    /// This is the high-level convenience wrapper around [`LinkSender::push`]. It does not perform
    /// serialization, reliability, fragmentation, encryption, or IO; those responsibilities live
    /// in higher-level protocol crates and concrete transport crates.
    pub fn send(&mut self, payload: SendPayload) {
        self.send.push(payload);
    }
}

/// Transport-observed statistics for a [`Link`].
///
/// These values are intentionally lightweight and transport-defined. Higher-level diagnostics can
/// combine them with replication/message metrics from other Lightyear crates.
#[derive(Default, Debug, Clone, Copy)]
pub struct LinkStats {
    /// Estimated round-trip time for this link.
    pub rtt: Duration,
    /// Estimated variation in packet delay for this link.
    pub jitter: Duration,
}

#[deprecated(note = "Use LinkSystems instead")]
/// Deprecated alias for [`LinkSystems`].
pub type LinkSet = LinkSystems;

/// System sets for `Link`-related operations.
///
/// These are used to order systems that handle:
/// - Receiving data from the IO layer into the `Link` buffer.
/// - Applying link conditioning to received packets.
/// - Sending data from the `Link` buffer to the IO layer.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum LinkSystems {
    // PreUpdate
    /// Receive bytes from IO backends and make them available through [`LinkReceiver`].
    Receive,

    // PostUpdate
    /// Flush queued [`SendPayload`] values from [`LinkSender`] to IO backends.
    Send,
}

#[deprecated(note = "Use LinkReceiveSystems instead")]
/// Deprecated alias for [`LinkReceiveSystems`].
pub type LinkReceiveSet = LinkReceiveSystems;

/// System sets that make up [`LinkSystems::Receive`].
///
/// Transport plugins should put their raw receive systems in [`BufferToLink`](Self::BufferToLink).
/// [`LinkPlugin`] runs [`ApplyConditioner`](Self::ApplyConditioner) afterwards so higher-level
/// systems see only packets whose simulated delay has elapsed.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum LinkReceiveSystems {
    /// Receive bytes from IO and push them into [`LinkReceiver`].
    ///
    /// If the receiver has a [`LinkConditioner`], transport systems should call
    /// [`LinkReceiver::push`] so the packet is delayed or dropped before it becomes available.
    BufferToLink,
    /// Move ready packets from [`LinkConditioner`] into [`LinkReceiver`].
    ApplyConditioner,
}

/// Entity event requesting that a transport start establishing a [`Link`].
///
/// `LinkStart` is transport-facing: A transport plugin observes this event for its
/// own link entities and inserts [`Linking`] or [`Linked`] when the connection progresses.
#[derive(EntityEvent)]
pub struct LinkStart {
    /// Entity that owns the [`Link`] to start.
    pub entity: Entity,
}

/// Why a [`Link`] was unlinked via [`Unlink`] or [`Unlinked`].
#[derive(Default, Debug, Clone, PartialEq, Eq, Reflect)]
pub enum UnlinkReason {
    /// The local side explicitly requested a disconnect.
    ClientRequested,
    /// The server shut down cleanly.
    ServerStopped,
    /// The transport encountered a fatal error and the link can no longer
    /// communicate with the peer.
    ///
    /// The inner string contains a transport-specific description of the error.
    TransportError(String),
    /// The remote peer closed the connection, with a provided reason.
    ///
    /// On the peer's side, this is interpreted as a [`UnlinkReason::ClientRequested`].
    ByPeer(String),
    /// A reason not covered by other variants.
    ///
    /// Used as an escape hatch for transports or application code that need to
    /// surface a custom disconnect cause.
    Other(String),
    /// The link has not yet been established.
    ///
    /// This is the initial state of a [`Link`] before any connection attempt.
    #[default]
    Initial,
}

impl core::fmt::Display for UnlinkReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            UnlinkReason::ServerStopped => f.write_str("Server stopped"),
            UnlinkReason::ClientRequested => f.write_str("Client requested"),
            UnlinkReason::TransportError(s) => write!(f, "Transport error: {s}"),
            UnlinkReason::ByPeer(s) => write!(f, "Disconnected by peer: {s}"),
            UnlinkReason::Other(s) => f.write_str(s),
            UnlinkReason::Initial => f.write_str("Not connected"),
        }
    }
}

/// Entity event requesting that a transport terminate a [`Link`].
///
/// [`LinkPlugin`] observes this event and inserts [`Unlinked`] with the provided reason. Concrete
/// transports can also observe it to close sockets, sessions, streams, or in-process channels.
#[derive(EntityEvent, Clone, Debug)]
pub struct Unlink {
    /// Entity that owns the [`Link`] to terminate.
    #[event_target]
    pub entity: Entity,
    /// Human-readable reason propagated to [`Unlinked::reason`].
    pub reason: UnlinkReason,
}

/// Marker component for a link whose transport connection is being established.
///
/// Inserting this component updates [`Link::state`] to [`LinkState::Linking`] and removes
/// [`Linked`] and [`Unlinked`]. If [`Linked`] is inserted in the same frame first, the hook leaves
/// the link linked to avoid regressing a completed connection back to the in-progress state.
#[derive(Component, Default, Debug)]
#[component(on_insert = Linking::on_insert)]
pub struct Linking;

impl Linking {
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        // If `Linked` got inserted at the same frame right after `Linking`, we don't want to
        // change the state or remove the `Linked` component.
        if world.get::<Linked>(context.entity).is_some() {
            return;
        }
        if let Some(mut link) = world.get_mut::<Link>(context.entity) {
            link.state = LinkState::Linking;
        }
        world
            .commands()
            .entity(context.entity)
            .remove::<(Linked, Unlinked)>();
    }
}

/// Marker component for an established link.
///
/// Inserting this component updates [`Link::state`] to [`LinkState::Linked`] and removes
/// [`Linking`] and [`Unlinked`].
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

/// Marker component for a link that is not connected.
///
/// Inserting this component updates [`Link::state`] to [`LinkState::Unlinked`] and removes
/// [`Linked`] and [`Linking`]. The optional [`reason`](Self::reason) is intended for diagnostics
/// and for transports that need to surface disconnect causes to application code.
#[derive(Component, Default, Debug)]
#[component(on_insert = Unlinked::on_insert)]
pub struct Unlinked {
    /// Human-readable disconnect or initial-state reason.
    pub reason: UnlinkReason,
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

/// Bevy plugin that installs link lifecycle and receive-buffer systems.
///
/// This plugin configures system sets for:
/// - receiving data into [`Link`] buffers via [`LinkSystems::Receive`];
/// - applying receive-side link conditioning via [`LinkReceiveSystems::ApplyConditioner`];
/// - sending data from [`Link`] buffers via [`LinkSystems::Send`].
///
/// Concrete transport plugins normally add this plugin, then schedule their IO systems inside
/// [`LinkReceiveSystems::BufferToLink`] and [`LinkSystems::Send`].
pub struct LinkPlugin;

impl LinkPlugin {
    /// Moves ready conditioned packets into each link's receive buffer.
    ///
    /// [`LinkReceiver::push`] stores packets in [`LinkConditioner`] when conditioning is enabled.
    /// This system polls those conditioners against [`Instant::now`] and appends packets whose
    /// simulated delivery time has elapsed. It is installed in
    /// [`LinkReceiveSystems::ApplyConditioner`] by [`LinkPlugin`].
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

    /// Handles [`Unlink`] requests by inserting [`Unlinked`].
    fn unlink(mut unlink: On<Unlink>, mut commands: Commands) {
        if let Ok(mut c) = commands.get_entity(unlink.entity) {
            c.insert(Unlinked {
                reason: core::mem::take(&mut unlink.reason),
            });
        }
    }
}

impl Plugin for LinkPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            Self::apply_link_conditioner.in_set(LinkReceiveSystems::ApplyConditioner),
        );
        app.configure_sets(
            PreUpdate,
            (
                LinkReceiveSystems::BufferToLink,
                LinkReceiveSystems::ApplyConditioner,
            )
                .in_set(LinkSystems::Receive)
                .chain(),
        );
        app.configure_sets(PostUpdate, LinkSystems::Send);

        app.add_observer(Self::unlink);
    }
}
