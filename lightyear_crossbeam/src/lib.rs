//! # Lightyear Crossbeam
//!
//! This crate provides a transport layer for Lightyear that uses `crossbeam-channel`.
//! It's primarily intended for local testing or scenarios where in-process message passing
//! is desired, simulating a network connection without actual network I/O.
//!
//! It defines `CrossbeamIo` for channel-based communication and `CrossbeamPlugin`
//! to integrate this transport into a Bevy application.
//!
//! ## Connection layer
//!
//! `CrossbeamPlugin` is a pure transport: it inserts `Linked` once `LinkStart` is triggered
//! but does not drive the `Connected` state. Pair it with a connection plugin to obtain a
//! full peer connection. For handshake-less use (e.g. local testing), pair it with
//! `lightyear_raw_connection::client::RawConnectionPlugin` and/or
//! `lightyear_raw_connection::server::RawConnectionPlugin` (with the corresponding `client`
//! / `server` feature enabled) and mark the relevant entities with `RawClient` /
//! `RawServer` so that `Linked` implies `Connected`. For authenticated use, pair it with
//! `lightyear_netcode` instead.
//!
//! ### Spawning crossbeam entities
//!
//! Always trigger `LinkStart` to bring a `CrossbeamIo` entity online, rather than
//! inserting `Linked` directly. `CrossbeamPlugin::link` (the observer driving
//! `LinkStart -> Linked`) gates on `With<CrossbeamIo>`, so by the time `Linked` lands the
//! `CrossbeamIo` is in place — and so are its required `LocalAddr` / `PeerAddr` — which
//! `RawConnectionPlugin`'s `Add<Linked>` observers read to construct
//! `LocalId` / `RemoteId`. Inserting `Linked` directly in the spawn bundle exposes a
//! window where those required components have not yet cascaded; the connection-layer
//! observer fails its query silently and the entity ends up `Linked` but never
//! `Connected`, so replication / message channels never wire up.
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
//!     .spawn((Client::default(), RawClient, Link::new(None), io))
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

/// Maximum transmission units; maximum size in bytes of a packet
pub(crate) const MTU: usize = 1472;
const LOCALHOST: SocketAddr = SocketAddr::new(core::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// A component that facilitates communication over `crossbeam-channel`.
///
/// This acts as a transport layer, allowing messages to be sent and received
/// via in-memory channels, simulating a network link. It holds the sender
/// and receiver ends of the channels.
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

    /// Create a pair of CrossbeamIo instances for local testing
    pub fn new_pair() -> (Self, Self) {
        let (sender1, receiver1) = crossbeam_channel::unbounded();
        let (sender2, receiver2) = crossbeam_channel::unbounded();

        (Self::new(sender1, receiver2), Self::new(sender2, receiver1))
    }
}

/// Bevy plugin to integrate the `CrossbeamIo` transport.
///
/// This plugin sets up the necessary systems for sending and receiving data
/// via `crossbeam-channel` when a `Link` component is present and active.
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

    /// Integration test for the documented server-side mirror pattern:
    /// pair `CrossbeamPlugin` with `lightyear_raw_connection`'s server
    /// `RawConnectionPlugin`, mark the parent server entity with `RawServer`,
    /// spawn the mirror with `LinkOf` + `CrossbeamIo`, then **trigger
    /// `LinkStart`** (rather than inserting `Linked` directly). The mirror
    /// should reach `Connected` with `ClientOf` so message / replication
    /// channels can wire up via downstream `On<Insert, (Transport, ClientOf)>`
    /// observers.
    ///
    /// Direct-`Linked` insertion in the same spawn bundle has been observed to
    /// race the `CrossbeamIo` required-component cascade (`PeerAddr`,
    /// `LocalAddr`) against `RawConnectionPlugin::on_link_of_linked`'s query;
    /// using `LinkStart` makes ordering deterministic.
    #[test]
    fn server_mirror_via_link_start_reaches_connected() {
        use lightyear_connection::prelude::Connected;
        use lightyear_connection::prelude::server::ClientOf;
        use lightyear_link::prelude::server::LinkOf;
        use lightyear_raw_connection::prelude::server::RawServer;

        let (_client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);
        app.add_plugins(lightyear_raw_connection::server::RawConnectionPlugin);

        // Server entity: RawServer marker (Server is auto-added via #[require]).
        let server_entity = app.world_mut().spawn(RawServer).id();

        // Mirror entity: LinkOf parented to the server, plus CrossbeamIo.
        // No Linked here — that's the whole point of the documented pattern.
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

        // Trigger LinkStart on the mirror. CrossbeamPlugin::link adds Linked,
        // RawConnectionPlugin::on_link_of_linked then adds Connected + ClientOf.
        app.world_mut().trigger(LinkStart {
            entity: mirror_entity,
        });

        app.update();

        let world = app.world();
        assert!(
            world.get::<Linked>(mirror_entity).is_some(),
            "mirror should have Linked after LinkStart fires CrossbeamPlugin::link"
        );
        assert!(
            world.get::<Connected>(mirror_entity).is_some(),
            "mirror should have Connected — RawConnectionPlugin's on_link_of_linked should fire \
             on Add<Linked> with PeerAddr already cascaded from CrossbeamIo"
        );
        assert!(
            world.get::<ClientOf>(mirror_entity).is_some(),
            "mirror should have ClientOf so downstream channel observers can fire"
        );
    }
}
