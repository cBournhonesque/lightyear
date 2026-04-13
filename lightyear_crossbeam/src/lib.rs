//! # Lightyear Crossbeam
//!
//! This crate provides a transport layer for Lightyear that uses `crossbeam-channel`.
//! It's primarily intended for local testing or scenarios where in-process message passing
//! is desired, simulating a network connection without actual network I/O.
//!
//! It defines `CrossbeamIo` for channel-based communication and `CrossbeamPlugin`
//! to integrate this transport into a Bevy application.
#![no_std]

extern crate alloc;

use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::error::Result;
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use bytes::Bytes;
use core::net::{Ipv4Addr, SocketAddr};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use lightyear_core::time::Instant;
use lightyear_connection::prelude::client::{Client, Connected};
use lightyear_connection::prelude::server::ClientOf;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::prelude::server::LinkOf;
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

    /// For crossbeam client entities, Linked implies Connected (no handshake needed).
    /// Mirrors `RawClient::on_linked` in `lightyear_raw_connection`.
    fn on_client_linked(
        trigger: On<Add, Linked>,
        query: Query<&LocalAddr, (With<CrossbeamIo>, With<Client>)>,
        mut commands: Commands,
    ) {
        if let Ok(local_addr) = query.get(trigger.entity) {
            trace!("CrossbeamIo client Linked! Adding Connected");
            commands.entity(trigger.entity).insert((
                Connected,
                LocalId(PeerId::Raw(local_addr.0)),
                RemoteId(PeerId::Server),
            ));
        }
    }

    /// For crossbeam server-side client mirror entities (LinkOf), Linked implies Connected.
    /// Mirrors `RawServer::on_link_of_linked` in `lightyear_raw_connection`.
    fn on_server_client_linked(
        trigger: On<Add, Linked>,
        query: Query<&PeerAddr, (With<CrossbeamIo>, With<LinkOf>)>,
        mut commands: Commands,
    ) {
        if let Ok(peer_addr) = query.get(trigger.entity) {
            trace!("CrossbeamIo server LinkOf Linked! Adding Connected + ClientOf");
            commands.entity(trigger.entity).insert((
                Connected,
                LocalId(PeerId::Server),
                RemoteId(PeerId::Raw(peer_addr.0)),
                ClientOf,
            ));
        }
    }

    fn send(mut query: Query<IOQuery, With<Linked>>) -> Result {
        for mut io in query.iter_mut() {
            for payload in io.link.send.drain() {
                #[cfg(feature = "test_utils")]
                if io.helper.is_some_and(|h| h.block_send) {
                    continue;
                }
                if io.crossbeam_io.sender.try_send(payload).is_err() {
                    // Channel disconnected (peer dropped) — not an error during shutdown.
                    // Remaining payloads are cleared when the Drain iterator drops on break.
                    trace!("CrossbeamIo send failed: channel disconnected");
                    break;
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
        app.add_observer(Self::on_client_linked);
        app.add_observer(Self::on_server_client_linked);
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
    use lightyear_connection::client::ConnectionPlugin;
    use lightyear_connection::prelude::client::Connect;
    use lightyear_link::prelude::server::LinkOf;
    use lightyear_link::prelude::Server;

    /// Verify that a crossbeam client reaches Connected after Connect trigger.
    #[test]
    fn client_reaches_connected_via_crossbeam() {
        let (client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(bevy_app::ScheduleRunnerPlugin::default());
        app.add_plugins(ConnectionPlugin);
        app.add_plugins(CrossbeamPlugin);

        // Spawn server entity
        let server_entity = app
            .world_mut()
            .spawn((Server::default(), Link::new(None)))
            .id();

        // Spawn server-side mirror with Linked (as the stepper does)
        app.world_mut().spawn((
            LinkOf {
                server: server_entity,
            },
            Link::new(None),
            Linked,
            server_io,
        ));

        // Spawn client entity
        let client_entity = app
            .world_mut()
            .spawn((Client::default(), Link::new(None), client_io))
            .id();

        // Trigger Connect (same as soup's handle_match_found_events)
        app.world_mut().trigger(Connect {
            entity: client_entity,
        });

        // Step a few frames for observers to fire
        for _ in 0..5 {
            app.update();
        }

        // Client should have Connected
        assert!(
            app.world().get::<Connected>(client_entity).is_some(),
            "Client entity should have Connected component after crossbeam Connect trigger"
        );
    }

    /// Verify that a server-side LinkOf entity with crossbeam gets Connected + ClientOf.
    #[test]
    fn server_mirror_reaches_connected_via_crossbeam() {
        let (_client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(bevy_app::ScheduleRunnerPlugin::default());
        app.add_plugins(ConnectionPlugin);
        app.add_plugins(CrossbeamPlugin);

        let server_entity = app
            .world_mut()
            .spawn((Server::default(), Link::new(None)))
            .id();

        // Spawn mirror entity with Linked (as soup's spawn_server_entity does)
        let mirror_entity = app
            .world_mut()
            .spawn((
                LinkOf {
                    server: server_entity,
                },
                Link::new(None),
                Linked,
                server_io,
            ))
            .id();

        for _ in 0..5 {
            app.update();
        }

        assert!(
            app.world().get::<Connected>(mirror_entity).is_some(),
            "Server mirror entity should have Connected"
        );
        assert!(
            app.world().get::<ClientOf>(mirror_entity).is_some(),
            "Server mirror entity should have ClientOf"
        );
    }

    /// Verify that dropping one end of the crossbeam pair doesn't panic the send system.
    #[test]
    fn send_after_peer_disconnect_does_not_panic() {
        let (client_io, server_io) = CrossbeamIo::new_pair();

        let mut app = App::new();
        app.add_plugins(bevy_app::ScheduleRunnerPlugin::default());
        app.add_plugins(ConnectionPlugin);
        app.add_plugins(CrossbeamPlugin);

        let server_entity = app
            .world_mut()
            .spawn((Server::default(), Link::new(None)))
            .id();

        // Spawn server mirror with crossbeam IO
        let mirror_entity = app
            .world_mut()
            .spawn((
                LinkOf {
                    server: server_entity,
                },
                Link::new(None),
                Linked,
                server_io,
            ))
            .id();

        // Spawn client and connect
        let client_entity = app
            .world_mut()
            .spawn((Client::default(), Link::new(None), client_io))
            .id();

        app.world_mut().trigger(Connect {
            entity: client_entity,
        });

        // Step to establish connection
        for _ in 0..3 {
            app.update();
        }

        // Queue some data on the client's send buffer
        if let Some(mut link) = app.world_mut().get_mut::<Link>(client_entity) {
            link.send.push(Bytes::from_static(b"hello"));
        }

        // Drop the server mirror entity (simulates server thread exit / shutdown)
        app.world_mut().despawn(mirror_entity);

        // Step — send system should handle the disconnected channel gracefully
        for _ in 0..3 {
            app.update();
        }

        // If we get here without panic, the test passes
    }
}
