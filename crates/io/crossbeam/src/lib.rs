//! In-process Crossbeam channel transport for Lightyear.
//!
//! This crate provides [`CrossbeamIo`], a transport implementation backed by
//! `crossbeam-channel`. It is primarily intended for tests, local examples, and in-process
//! simulations where deterministic setup and low overhead are more useful than real network IO.
//! It still uses Lightyear's normal [`Link`] buffers and lifecycle markers, so code above the
//! transport layer can be exercised without special cases.
//!
//! ## Connection layer
//!
//! [`CrossbeamPlugin`] is a pure transport: it inserts [`Linked`] once [`LinkStart`] is triggered
//! but does not drive the `Connected` state. Pair it with a connection plugin to obtain a full peer
//! connection. For handshake-less use, pair it with
//! `lightyear_raw_connection::client::RawConnectionPlugin` and/or
//! `lightyear_raw_connection::server::RawConnectionPlugin` and mark entities with `RawClient` /
//! `RawServer` so that [`Linked`] implies `Connected`. For authenticated use, pair it with
//! `lightyear_netcode`.
//!
//! ### Spawning crossbeam entities
//!
//! Always trigger [`LinkStart`] to bring a [`CrossbeamIo`] entity online, rather than inserting
//! [`Linked`] directly. [`CrossbeamPlugin`] gates its link observer on `With<CrossbeamIo>`, so by
//! the time [`Linked`] is inserted the required Aeronet-compatible `LocalAddr` and `PeerAddr`
//! components are also present. Connection-layer `Add<Linked>` observers can then construct their
//! local and remote IDs reliably.
//!
//! ```ignore
//! // Server-side mirror entity (one per connecting crossbeam client):
//! let mirror = commands
//!     .spawn((LinkOf { server }, Link::new(None), io))
//!     .id();
//! commands.trigger(LinkStart { entity: mirror });
//!
//! // Client-side connection entity:
//! let client = commands
//!     .spawn((Client, RawClient, Link::new(None), io))
//!     .id();
//! commands.trigger(Connect { entity: client });  // Connect → LinkStart internally
//! ```
#![no_std]

extern crate alloc;

use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use bytes::Bytes;
use core::net::{Ipv4Addr, SocketAddr};
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use lightyear_core::time::Instant;
use lightyear_link::{Link, LinkPlugin, LinkReceiveSystems, LinkStart, LinkSystems, Linked};
use tracing::{error, trace};

/// Maximum payload size used by Lightyear's packet transports.
pub(crate) const MTU: usize = 1472;
const LOCALHOST: SocketAddr = SocketAddr::new(core::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// In-process transport component backed by `crossbeam-channel`.
///
/// `CrossbeamIo` is inserted on a Lightyear link entity and requires a [`Link`], `LocalAddr`, and
/// `PeerAddr`. The addresses are dummy localhost values used by connection-layer code that expects
/// address components even though no real socket exists.
///
/// Use [`new_pair`](Self::new_pair) for the normal bidirectional setup: one returned component goes
/// on the client-side entity and the other on the server-side mirror entity.
#[derive(Component, Clone)]
#[require(Link::new(None))]
#[require(LocalAddr(LOCALHOST))]
#[require(PeerAddr(LOCALHOST))]
pub struct CrossbeamIo {
    sender: Sender<Bytes>,
    receiver: Receiver<Bytes>,
}

impl CrossbeamIo {
    /// Build a `CrossbeamIo` from caller-provided channel ends.
    ///
    /// The sender must be backed by an **unbounded** channel
    /// (`crossbeam_channel::unbounded()`). Wiring in a bounded sender is
    /// allowed by the type system but will trigger a re-queue + error log
    /// on backpressure inside the send system, since the transport has no
    /// way to apply flow control to upstream callers. Use `new_pair` for
    /// the canonical configuration.
    pub fn new(sender: Sender<Bytes>, receiver: Receiver<Bytes>) -> Self {
        Self { sender, receiver }
    }

    /// Creates two cross-connected [`CrossbeamIo`] instances.
    ///
    /// Payloads sent by the first instance are received by the second, and payloads sent by the
    /// second are received by the first. Both directions use unbounded channels, matching the
    /// assumptions documented by [`new`](Self::new).
    pub fn new_pair() -> (Self, Self) {
        let (sender1, receiver1) = crossbeam_channel::unbounded();
        let (sender2, receiver2) = crossbeam_channel::unbounded();

        (Self::new(sender1, receiver2), Self::new(sender2, receiver1))
    }
}

/// Bevy plugin that integrates [`CrossbeamIo`] with Lightyear links.
///
/// The plugin installs:
/// - a [`LinkStart`] observer that immediately marks [`CrossbeamIo`] entities as [`Linked`];
/// - a receive system in [`LinkReceiveSystems::BufferToLink`] that drains channel payloads into
///   [`Link::recv`];
/// - a send system in [`LinkSystems::Send`] that flushes [`Link::send`] into the channel.
///
/// It does not implement authentication, handshake state, or `Connected`; pair it with a Lightyear
/// connection plugin when higher-level connection state is needed.
pub struct CrossbeamPlugin;

#[derive(QueryData)]
#[query_data(mutable)]
struct IOQuery {
    entity: Entity,
    link: &'static mut Link,
    crossbeam_io: &'static CrossbeamIo,
    #[cfg(feature = "test_utils")]
    helper: Option<&'static lightyear_core::test::TestHelper>,
}

impl CrossbeamPlugin {
    fn link(
        link_start: On<LinkStart>,
        query: Query<(), With<CrossbeamIo>>,
        mut commands: Commands,
    ) {
        if query.get(link_start.entity).is_ok() {
            trace!(
                "Immediately add Linked for CrossbeamIO entity: {:?}",
                link_start.entity
            );
            commands.entity(link_start.entity).insert(Linked);
        }
    }

    fn send(mut query: Query<IOQuery, With<Linked>>) {
        // Iterate via `pop` so that `Full` can re-queue the failed payload
        // without losing the rest of the batch.
        for mut io in query.iter_mut() {
            let entity = io.entity;
            while let Some(payload) = io.link.send.pop() {
                #[cfg(feature = "test_utils")]
                if io.helper.is_some_and(|h| h.block_send) {
                    // Drop this payload only; keep retrying the rest.
                    continue;
                }
                match io.crossbeam_io.sender.try_send(payload) {
                    Ok(()) => {}
                    Err(TrySendError::Disconnected(_)) => {
                        // Peer dropped — not an error during shutdown. Clear the
                        // rest of this entity's send queue so we don't keep
                        // retrying every frame.
                        trace!(
                            "CrossbeamIo send dropped on entity {entity:?}: channel disconnected"
                        );
                        let _ = io.link.send.drain();
                        break;
                    }
                    Err(TrySendError::Full(p)) => {
                        // Defensive backstop: `CrossbeamIo::new` documents that
                        // it requires an unbounded sender. Push to the front so
                        // FIFO is preserved across the still-queued payloads.
                        error!(
                            "CrossbeamIo send: channel full on entity {entity:?} (transport assumes unbounded); re-queueing"
                        );
                        io.link.send.push_front(p);
                        break;
                    }
                }
            }
        }
    }

    fn receive(mut query: Query<(&mut Link, &mut CrossbeamIo), With<Linked>>) {
        query.par_iter_mut().for_each(|(mut link, crossbeam_io)| {
            // Try to receive all available messages
            loop {
                match crossbeam_io.receiver.try_recv() {
                    Ok(data) => {
                        trace!("recv data: {data:?}");
                        link.recv.push(data, Instant::now())
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        error!("CrossbeamIO channel is disconnected");
                        break;
                    }
                }
            }
        })
    }
}

impl Plugin for CrossbeamPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_observer(Self::link);
        app.add_systems(
            PreUpdate,
            Self::receive.in_set(LinkReceiveSystems::BufferToLink),
        );
        app.add_systems(PostUpdate, Self::send.in_set(LinkSystems::Send));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the send system tolerates a disconnected peer channel without
    /// panicking, and that Drain clears the queued payloads on break.
    /// This is a pure transport-layer test — no connection plugin is needed.
    #[test]
    fn send_after_peer_disconnect_does_not_panic() {
        let (client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        // Spawn the sender side as Linked (skipping the LinkStart trigger keeps
        // the test independent of any connection plugin).
        let sender_entity = app
            .world_mut()
            .spawn((Link::new(None), Linked, client_io))
            .id();

        // Drop the peer side of the channel pair before sending.
        drop(server_io);

        // Queue two payloads; the send system should handle Disconnected
        // gracefully and the Drain on break should clear both.
        let mut link = app
            .world_mut()
            .get_mut::<Link>(sender_entity)
            .expect("sender entity should have Link");
        link.send.push(Bytes::from_static(b"hello"));
        link.send.push(Bytes::from_static(b"world"));

        app.update();

        let link = app
            .world()
            .get::<Link>(sender_entity)
            .expect("sender entity should still have Link");
        assert_eq!(
            link.send.len(),
            0,
            "Drain should clear queued payloads on disconnect"
        );
    }

    /// Verify multi-payload round-trip send/receive between a paired client and
    /// server transport, including FIFO ordering.
    #[test]
    fn round_trip_send_receive() {
        let (client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        let client_entity = app
            .world_mut()
            .spawn((Link::new(None), Linked, client_io))
            .id();
        let server_entity = app
            .world_mut()
            .spawn((Link::new(None), Linked, server_io))
            .id();

        let mut client_link = app
            .world_mut()
            .get_mut::<Link>(client_entity)
            .expect("client entity should have Link");
        client_link.send.push(Bytes::from_static(b"a"));
        client_link.send.push(Bytes::from_static(b"b"));
        client_link.send.push(Bytes::from_static(b"c"));

        // Two frames: frame 1 client.PostUpdate `send` pushes into the channel,
        // frame 2 server.PreUpdate `receive` pulls them into Link.recv.
        app.update();
        app.update();

        let mut server_link = app
            .world_mut()
            .get_mut::<Link>(server_entity)
            .expect("server entity should have Link");
        let p1 = server_link.recv.pop().expect("first payload missing");
        let p2 = server_link.recv.pop().expect("second payload missing");
        let p3 = server_link.recv.pop().expect("third payload missing");
        assert_eq!(p1.as_ref(), b"a");
        assert_eq!(p2.as_ref(), b"b");
        assert_eq!(p3.as_ref(), b"c");
        assert!(
            server_link.recv.pop().is_none(),
            "no extra payloads should be received"
        );
    }

    /// `CrossbeamIo` is documented to require unbounded channels, but the send
    /// system has a defensive re-queue path for callers who wire in a bounded
    /// `Sender` via `CrossbeamIo::new`. Verify that path: a payload that hits
    /// `TrySendError::Full` lands back at the front of the entity's send queue
    /// (preserving FIFO across the still-queued payloads) rather than being
    /// silently dropped or shuffled.
    #[test]
    fn send_with_bounded_channel_requeues_on_full() {
        let (bounded_sender, _peer_recv_unread) = crossbeam_channel::bounded::<Bytes>(1);
        let (_, dummy_recv) = crossbeam_channel::unbounded::<Bytes>();
        let client_io = CrossbeamIo::new(bounded_sender, dummy_recv);

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        let client_entity = app
            .world_mut()
            .spawn((Link::new(None), Linked, client_io))
            .id();

        // Capacity 1: "first" fills the channel, "second" hits Full and must
        // be re-queued at the front so it precedes "third" on the next frame.
        let mut link = app
            .world_mut()
            .get_mut::<Link>(client_entity)
            .expect("client entity should have Link");
        link.send.push(Bytes::from_static(b"first"));
        link.send.push(Bytes::from_static(b"second"));
        link.send.push(Bytes::from_static(b"third"));

        app.update();

        let mut link = app
            .world_mut()
            .get_mut::<Link>(client_entity)
            .expect("client entity should still have Link");
        assert_eq!(
            link.send.len(),
            2,
            "Full payload should be re-queued (queue starts at 3, 1 sent, 1 Full re-queued)"
        );
        assert_eq!(
            link.send
                .pop()
                .expect("re-queued Full payload missing")
                .as_ref(),
            b"second",
            "Full payload should land at front of queue to preserve FIFO"
        );
        assert_eq!(
            link.send.pop().expect("third payload missing").as_ref(),
            b"third",
            "still-queued payloads should follow the re-queued one"
        );
    }

    /// Pair `CrossbeamPlugin` with server-side `RawConnectionPlugin`, mark the
    /// parent with `RawServer`, spawn the mirror with `LinkOf` + `CrossbeamIo`,
    /// then trigger `LinkStart` (not direct `Linked` insertion). The mirror
    /// should reach `Linked + Connected + ClientOf` so downstream
    /// `On<Insert, (Transport, ClientOf)>` channel observers fire.
    #[test]
    fn server_mirror_via_link_start_reaches_connected() {
        use lightyear_connection::prelude::Connected;
        use lightyear_connection::prelude::server::ClientOf;
        use lightyear_link::prelude::server::LinkOf;
        use lightyear_raw_connection::prelude::server::RawServer;

        // Keep the client side of the pair alive for the duration of the test —
        // dropping it would Disconnect the server's channel before the assert.
        let (_client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);
        app.add_plugins(lightyear_raw_connection::server::RawConnectionPlugin);

        let server_entity = app.world_mut().spawn(RawServer).id();
        let mirror_entity = app
            .world_mut()
            .spawn((
                LinkOf {
                    server: server_entity,
                },
                Link::new(None),
                server_io,
            ))
            .id();
        app.world_mut().trigger(LinkStart {
            entity: mirror_entity,
        });

        app.update();

        let world = app.world();
        assert!(world.get::<Linked>(mirror_entity).is_some(), "Linked");
        assert!(world.get::<Connected>(mirror_entity).is_some(), "Connected");
        assert!(world.get::<ClientOf>(mirror_entity).is_some(), "ClientOf");
    }
}
