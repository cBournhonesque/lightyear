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
//! `lightyear_raw_connection`'s `RawConnectionPlugin` and mark the relevant entities with
//! `RawClient` / `RawServer` so that `Linked` implies `Connected`. For authenticated use,
//! pair it with `lightyear_netcode` instead.
#![no_std]

extern crate alloc;

use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::error::Result;
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

    fn send(mut query: Query<IOQuery, With<Linked>>) -> Result {
        for mut io in query.iter_mut() {
            for payload in io.link.send.drain() {
                #[cfg(feature = "test_utils")]
                if io.helper.is_some_and(|h| h.block_send) {
                    // Skip this payload only; keep draining the rest for this entity.
                    continue;
                }
                match io.crossbeam_io.sender.try_send(payload) {
                    Ok(()) => {}
                    Err(TrySendError::Disconnected(_)) => {
                        // Peer dropped — not an error during shutdown. Remaining
                        // payloads for this entity are cleared when the Drain
                        // iterator drops on break.
                        trace!("CrossbeamIo send dropped: channel disconnected");
                        break;
                    }
                    Err(TrySendError::Full(_)) => {
                        // CrossbeamIo channels are constructed unbounded, so this
                        // is unreachable unless a caller wires in a bounded sender
                        // via CrossbeamIo::new. Drop the batch loudly.
                        error!(
                            "CrossbeamIo send dropped: channel full (transport assumes unbounded)"
                        );
                        break;
                    }
                }
            }
        }
        Ok(())
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
        if let Some(mut link) = app.world_mut().get_mut::<Link>(sender_entity) {
            link.send.push(Bytes::from_static(b"hello"));
            link.send.push(Bytes::from_static(b"world"));
        }

        for _ in 0..3 {
            app.update();
        }

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

        if let Some(mut link) = app.world_mut().get_mut::<Link>(client_entity) {
            link.send.push(Bytes::from_static(b"a"));
            link.send.push(Bytes::from_static(b"b"));
            link.send.push(Bytes::from_static(b"c"));
        }

        for _ in 0..3 {
            app.update();
        }

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
}
