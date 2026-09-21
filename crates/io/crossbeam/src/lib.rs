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
//! [`CrossbeamPlugin`] admits debug connections automatically after [`LinkStart`]. Explicit
//! connection backends such as `lightyear_netcode` retain control of their own admission.
//! Insert `NoDefaultConnection` to use only the transport lifecycle.
//!
//! ### Spawning crossbeam entities
//!
//! Always trigger [`LinkStart`] to bring a [`CrossbeamIo`] entity online, rather than inserting
//! [`Linked`] directly. Native fallback IDs are World-local entity IDs, not an agreement with the
//! other end of the channel. Tests may preinsert coordinated `LocalId` and `RemoteId` values.
//!
//! ```ignore
//! // Server-side mirror entity (one per connecting crossbeam client):
//! let mirror = commands
//!     .spawn((LinkOf { endpoint: server }, Link::default(), io))
//!     .id();
//! commands.trigger(LinkStart { entity: mirror });
//!
//! // Client-side connection entity:
//! let client = commands
//!     .spawn((Client, Link::default(), io))
//!     .id();
//! commands.trigger(Connect { entity: client });  // Connect → LinkStart internally
//! ```
#![no_std]

extern crate alloc;

use alloc::string::String;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use lightyear_connection::admission::{DefaultConnectionPlugin, apply_default_ids};
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_core::time::Instant;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSystems, LinkStart, LinkSystems, Linked, Unlink, UnlinkReason,
    recv_payload_from_bytes,
};
use tracing::{error, trace};

/// Maximum payload size used by Lightyear's packet transports.
pub(crate) const MTU: usize = 1472;

/// In-process transport component backed by `crossbeam-channel`.
///
/// `CrossbeamIo` is inserted on a Lightyear link entity and requires a [`Link`].
/// Channel links have no network addresses.
///
/// Use [`new_pair`](Self::new_pair) for the normal bidirectional setup: one returned component goes
/// on the client-side entity and the other on the server-side mirror entity.
#[derive(Component, Clone)]
#[require(Link::default())]
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
/// Debug connections are admitted automatically; explicit connection backends remain responsible
/// for authentication and admission.
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

    fn admit_link(
        trigger: On<Add, (Linked, CrossbeamIo)>,
        query: Query<(), (With<Linked>, With<CrossbeamIo>)>,
        mut commands: Commands,
    ) {
        if query.get(trigger.entity).is_ok() {
            let entity = trigger.entity;
            commands.queue(move |world: &mut World| {
                // Channels expose no shared identity; use a local routing label.
                let candidate = PeerId(entity.to_bits().max(1));
                apply_default_ids(world, entity, LocalId(candidate), RemoteId(candidate));
            });
        }
    }

    fn unlink(trigger: On<Unlink>, mut commands: Commands) {
        // Dropping the channel ends lets the other peer observe disconnection as well.
        let entity = trigger.entity;
        commands.queue(move |world: &mut World| {
            if let Ok(mut entity) = world.get_entity_mut(entity) {
                entity.remove::<CrossbeamIo>();
            }
        });
    }

    fn send(mut query: Query<IOQuery, With<Linked>>, mut commands: Commands) {
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
                        commands.trigger(Unlink {
                            entity,
                            reason: UnlinkReason::TransportError(String::from(
                                "Crossbeam channel disconnected",
                            )),
                        });
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

    fn receive(
        mut query: Query<(Entity, &mut Link, &CrossbeamIo), With<Linked>>,
        mut commands: Commands,
    ) {
        for (entity, mut link, crossbeam_io) in query.iter_mut() {
            // Try to receive all available messages
            loop {
                match crossbeam_io.receiver.try_recv() {
                    Ok(data) => {
                        trace!("recv data: {data:?}");
                        link.recv
                            .push(recv_payload_from_bytes(data), Instant::now())
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        trace!(
                            "CrossbeamIo receive dropped on entity {entity:?}: channel disconnected"
                        );
                        commands.trigger(Unlink {
                            entity,
                            reason: UnlinkReason::TransportError(String::from(
                                "Crossbeam channel disconnected",
                            )),
                        });
                        break;
                    }
                }
            }
        }
    }
}

impl Plugin for CrossbeamPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        if !app.is_plugin_added::<DefaultConnectionPlugin>() {
            app.add_plugins(DefaultConnectionPlugin);
        }
        app.add_observer(Self::link);
        app.add_observer(Self::admit_link);
        app.add_observer(Self::unlink);
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
    use lightyear_connection::prelude::{Connected, Disconnected};
    use lightyear_link::LinkState;

    /// Verify the send system tolerates a disconnected peer channel without
    /// panicking, clears the queued payloads, and marks the link as unlinked.
    /// This is a pure transport-layer test — no connection plugin is needed.
    #[test]
    fn send_after_peer_disconnect_unlinks_transport() {
        let (sender, peer_receiver) = crossbeam_channel::unbounded::<Bytes>();
        let (peer_sender, receiver) = crossbeam_channel::unbounded::<Bytes>();
        let client_io = CrossbeamIo::new(sender, receiver);

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        // Spawn the sender side as Linked (skipping the LinkStart trigger keeps
        // the test independent of any connection plugin).
        let sender_entity = app
            .world_mut()
            .spawn((Link::default(), Linked, client_io))
            .id();

        // Drop only the peer receiver before sending. Keep `peer_sender`
        // alive so the receive system sees `Empty`, not `Disconnected`,
        // and this test isolates the send-side unlink path.
        drop(peer_receiver);

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
        assert_eq!(link.state, LinkState::Unlinked);
        assert!(app.world().get::<Linked>(sender_entity).is_none());
        assert!(
            app.world()
                .get::<lightyear_link::Unlinked>(sender_entity)
                .is_some()
        );
        drop(peer_sender);
    }

    /// Verify the receive side also marks the link as unlinked when the peer
    /// channel is dropped before any local send attempt observes it.
    #[test]
    fn receive_after_peer_disconnect_unlinks_transport() {
        let (client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        let client_entity = app
            .world_mut()
            .spawn((Link::default(), Linked, client_io))
            .id();

        drop(server_io);

        app.update();

        let link = app
            .world()
            .get::<Link>(client_entity)
            .expect("client entity should still have Link");
        assert_eq!(link.state, LinkState::Unlinked);
        assert!(app.world().get::<Linked>(client_entity).is_none());
        assert!(
            app.world()
                .get::<lightyear_link::Unlinked>(client_entity)
                .is_some()
        );
    }

    /// Channel closure propagates through the default connection lifecycle.
    #[test]
    fn peer_disconnect_disconnects_default_client() {
        use lightyear_connection::client::{Client, Connect};

        let (client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        let client_entity = app.world_mut().spawn((Client, client_io)).id();
        app.world_mut().trigger(Connect {
            entity: client_entity,
        });
        app.update();

        assert!(app.world().get::<Linked>(client_entity).is_some());
        assert!(app.world().get::<Connected>(client_entity).is_some());
        assert!(app.world().get::<Disconnected>(client_entity).is_none());

        drop(server_io);

        app.update();

        assert!(app.world().get::<Linked>(client_entity).is_none());
        assert!(app.world().get::<Connected>(client_entity).is_none());
        assert!(app.world().get::<Disconnected>(client_entity).is_some());
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
            .spawn((Link::default(), Linked, client_io))
            .id();
        let server_entity = app
            .world_mut()
            .spawn((Link::default(), Linked, server_io))
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
        let (_dummy_sender, dummy_recv) = crossbeam_channel::unbounded::<Bytes>();
        let client_io = CrossbeamIo::new(bounded_sender, dummy_recv);

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        let client_entity = app
            .world_mut()
            .spawn((Link::default(), Linked, client_io))
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

    /// A bare server mirror reaches the connection layer without backend markers.
    #[test]
    fn server_mirror_via_link_start_reaches_connected() {
        use lightyear_connection::client_of::ClientOf;
        use lightyear_connection::prelude::Connected;
        use lightyear_link::prelude::{LinkOf, Server};

        // Keep the client side of the pair alive for the duration of the test —
        // dropping it would Disconnect the server's channel before the assert.
        let (_client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);

        let server_entity = app.world_mut().spawn(Server::default()).id();
        let mirror_entity = app
            .world_mut()
            .spawn((
                LinkOf {
                    endpoint: server_entity,
                },
                Link::default(),
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

        let (_other_client, other_server_io) = CrossbeamIo::new_pair();
        let other_mirror = app
            .world_mut()
            .spawn((
                LinkOf {
                    endpoint: server_entity,
                },
                other_server_io,
            ))
            .id();
        app.world_mut().trigger(LinkStart {
            entity: other_mirror,
        });
        app.update();
        assert!(app.world().get::<Connected>(other_mirror).is_some());
        assert_ne!(
            app.world().get::<RemoteId>(mirror_entity),
            app.world().get::<RemoteId>(other_mirror),
        );
    }

    #[test]
    fn explicit_backend_and_opt_out_stay_transport_only() {
        use lightyear_connection::admission::NoDefaultConnection;
        #[derive(Component)]
        #[require(NoDefaultConnection)]
        struct Explicit;
        let (explicit_io, opted_out_io) = CrossbeamIo::new_pair();
        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);
        let explicit = app.world_mut().spawn((explicit_io, Explicit)).id();
        let opted_out = app
            .world_mut()
            .spawn((opted_out_io, NoDefaultConnection))
            .id();
        app.world_mut().trigger(LinkStart { entity: explicit });
        app.world_mut().trigger(LinkStart { entity: opted_out });
        app.update();
        for entity in [explicit, opted_out] {
            assert!(app.world().get::<Linked>(entity).is_some());
            assert!(app.world().get::<Connected>(entity).is_none());
            assert!(app.world().get::<RemoteId>(entity).is_none());
            assert!(app.world().get::<Disconnected>(entity).is_none());
        }
    }

    #[test]
    fn requested_debug_ids_survive_until_disconnect_releases_both_peers() {
        use lightyear_connection::client::Disconnect;
        use lightyear_connection::network_topology::NetworkingMetadata;
        let (left_io, right_io) = CrossbeamIo::new_pair();
        let mut app = App::new();
        app.add_plugins(CrossbeamPlugin);
        let left = app
            .world_mut()
            .spawn((left_io, LocalId(PeerId(10)), RemoteId(PeerId(20))))
            .id();
        let right = app
            .world_mut()
            .spawn((right_io, LocalId(PeerId(20)), RemoteId(PeerId(10))))
            .id();
        app.world_mut().trigger(LinkStart { entity: left });
        app.world_mut().trigger(LinkStart { entity: right });
        app.update();
        assert_eq!(app.world().get::<LocalId>(left), Some(&LocalId(PeerId(10))));
        assert_eq!(
            app.world().get::<RemoteId>(left),
            Some(&RemoteId(PeerId(20)))
        );
        assert!(app.world().get::<Connected>(left).is_some());
        assert!(app.world().get::<Connected>(right).is_some());
        app.world_mut().trigger(Disconnect { entity: left });
        app.update();
        for entity in [left, right] {
            assert!(app.world().get::<Connected>(entity).is_none());
            assert!(app.world().get::<Linked>(entity).is_none());
            assert!(app.world().get::<Disconnected>(entity).is_some());
        }
        assert!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .is_empty()
        );
    }
}
