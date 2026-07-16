//! Multi-client UDP server transport.
//!
//! [`ServerUdpIo`](crate::server::ServerUdpIo) owns one non-blocking UDP socket bound to a
//! [`LocalAddr`](aeronet_io::connection::LocalAddr). Incoming datagrams are grouped by remote
//! [`SocketAddr`](core::net::SocketAddr), and each remote address is represented by a child
//! Lightyear [`Link`](lightyear_link::Link) related to the server entity through
//! [`LinkOf`](lightyear_link::prelude::LinkOf). This keeps server fan-out compatible with the
//! generic [`Server`](lightyear_link::server::Server) relationship model while preserving UDP's
//! connectionless socket model.

extern crate alloc;

use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::RelationshipTarget;
use bevy_ecs::system::ParallelCommands;
use tracing::{debug, error, info};

use crate::UdpError;
use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_platform::collections::{HashMap, hash_map::Entry};
use bytes::{BufMut, BytesMut};
use core::net::SocketAddr;
use lightyear_core::time::Instant;
use lightyear_link::prelude::{LinkOf, Server};
use lightyear_link::{Link, LinkPlugin, LinkStart, LinkSystems, Linked, Linking, Unlink, Unlinked};

/// Maximum UDP payload size used by this transport.
///
/// The value is chosen to avoid common IPv4 fragmentation limits. See
/// <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>.
pub(crate) const MTU: usize = 1472;

/// UDP server endpoint component.
///
/// Insert this on a Lightyear server entity. A [`LocalAddr`] component is required before
/// [`LinkStart`] is triggered; the plugin binds one socket to that address and creates child link
/// entities for remote addresses as datagrams arrive.
///
/// Each child link receives [`PeerAddr`] for its remote socket address and [`UdpLinkOfIO`] to mark
/// it as owned by this UDP server transport.
#[derive(Component)]
#[require(Server)]
pub struct ServerUdpIo {
    socket: Option<std::net::UdpSocket>,
    buffer: BytesMut,
    connected_addresses: HashMap<SocketAddr, LinkOfStatus>,
}

/// Marker for child link entities owned by [`ServerUdpIo`].
///
/// Server send systems use this marker to distinguish UDP-owned [`LinkOf`] children from child
/// links that may belong to another transport attached to the same server entity.
#[derive(Component)]
pub struct UdpLinkOfIO;

#[derive(Debug)]
enum LinkOfStatus {
    // we just received a packet from a new address and are in the process of spawning a new entity
    // to avoid race conditions, other connection packets from that address will be dropped for the rest of the frame
    //
    // we also won't process packets for this entity this frame, but only on the next frame (which is ok because the
    // client should be sending multiple connection packets)
    Spawning(Entity),
    // the link has been created
    Spawned(Entity),
}

impl Default for ServerUdpIo {
    fn default() -> Self {
        ServerUdpIo {
            socket: None,
            buffer: BytesMut::with_capacity(MTU),
            connected_addresses: HashMap::with_capacity(1),
        }
    }
}

/// Bevy plugin that integrates multi-client UDP server IO with Lightyear links.
///
/// The plugin installs:
/// - a [`LinkStart`] observer that binds the server socket and marks the server [`Linked`];
/// - an [`Unlink`] observer that closes the socket;
/// - a receive system that creates or finds a child link for each remote address and queues the
///   datagram in that child [`Link::recv`];
/// - a send system that drains each UDP child [`Link::send`] to its [`PeerAddr`].
pub struct ServerUdpPlugin;

impl ServerUdpPlugin {
    // TODO: we don't want this system to panic on error
    fn link(
        trigger: On<LinkStart>,
        mut query: Query<
            (&mut ServerUdpIo, Option<&LocalAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((mut udp_io, local_addr)) = query.get_mut(trigger.entity) {
            let local_addr = local_addr.ok_or(UdpError::LocalAddrMissing)?.0;
            info!("Server UDP socket bound to {}", local_addr);
            let socket = std::net::UdpSocket::bind(local_addr)?;
            socket.set_nonblocking(true)?;
            udp_io.socket = Some(socket);
            commands.entity(trigger.entity).insert(Linked);
        }
        Ok(())
    }

    fn unlink(trigger: On<Unlink>, mut query: Query<&mut ServerUdpIo, Without<Unlinked>>) {
        if let Ok(mut udp_io) = query.get_mut(trigger.entity) {
            info!("Server UDP socket closed");
            udp_io.socket = None;
        }
    }

    fn send(
        mut server_query: Query<(&mut ServerUdpIo, &Server), With<Linked>>,
        mut link_query: Query<(&mut Link, &PeerAddr), With<UdpLinkOfIO>>,
    ) {
        // TODO: parallelize
        server_query
            .iter_mut()
            .for_each(|(mut server_udp_io, server)| {
                server.collection().iter().for_each(|client_entity| {
                    let Some((mut link, remote_addr)) = link_query.get_mut(*client_entity).ok()
                    else {
                        // Not all server links are Udp Links, so we might not want this to ever print
                        debug!("Client entity {} not found in link query", client_entity);
                        return;
                    };

                    link.send.drain().for_each(|send_payload| {
                        server_udp_io
                            .socket
                            .as_mut()
                            .unwrap()
                            .send_to(send_payload.as_ref(), remote_addr.0)
                            .inspect_err(|e| {
                                error!("Error sending UDP packet to {}: {}", remote_addr.0, e);
                            })
                            .ok();
                    });
                });
            });
    }

    fn receive(
        commands: ParallelCommands,
        mut server_query: Query<(Entity, &mut ServerUdpIo), With<Linked>>,
        // TODO: we want to have With<Linked> here, but that would mean that if a client sends 2 packets in a row
        //  for the first one we spawn them, and for the second one the query will return False.
        //  maybe have a separate Vec for new addresses, and for these we don't require Linked?
        link_query: Query<Option<&mut Link>>,
    ) {
        server_query
            // TODO: would par_iter_mut be better here?
            .iter_mut()
            .for_each(|(server_entity, mut server_udp_io)| {
                // SAFETY: we know that each ServerUdpIo will target different Link entities, so there won't be any aliasing
                let mut link_query = unsafe { link_query.reborrow_unsafe() };

                // enable split borrows
                let server_udp_io = &mut *server_udp_io;

                loop {
                    // reserve additional space in the buffer
                    // this tries to reclaim space at the start of the buffer if possible
                    server_udp_io.buffer.reserve(crate::MTU);
                    // Check how much actual uninitialized space we have at the end
                    let capacity = server_udp_io.buffer.capacity();
                    let current_len = server_udp_io.buffer.len();
                    assert_eq!(current_len, 0);
                    let available_uninit = capacity - current_len;
                    let max_recv_len = core::cmp::min(available_uninit, crate::MTU);

                    // We get a raw pointer to the start of the uninitialized region.
                    // SAFETY: we know we have enough space to receive the data because we just reserved it
                    let buf_slice: &mut [u8] = unsafe {
                        let ptr = server_udp_io.buffer.as_mut_ptr().add(current_len);
                        core::slice::from_raw_parts_mut(ptr, max_recv_len)
                    };
                    match server_udp_io.socket.as_mut().unwrap().recv_from(buf_slice) {
                        Ok((recv_len, address)) => {
                            // Mark the received bytes as initialized
                            // SAFETY: we know that the buffer is large enough to hold the received data.
                            unsafe {
                                server_udp_io.buffer.advance_mut(recv_len);
                            }
                            let payload = server_udp_io.buffer.split_to(recv_len).freeze();
                            match server_udp_io.connected_addresses.entry(address) {
                                Entry::Occupied(mut entry) => {
                                    match *entry.get_mut() {
                                        LinkOfStatus::Spawning(_) => {
                                            // we are still spawning the entity, so we will drop this packet
                                            // and wait for the next one
                                            continue;
                                        }
                                        LinkOfStatus::Spawned(entity) => {
                                            match link_query.get_mut(entity) {
                                                Ok(mut link) => {
                                                    match link.as_mut() {
                                                        None => {
                                                            debug!("despawning entity {} because it has no udp link", entity);
                                                            // the entity exists but has not link.
                                                            // this is a weird state, let's despawn it
                                                            entry.remove();
                                                            commands.command_scope(|mut c| {
                                                                if let Ok(mut e) = c.get_entity(entity) {
                                                                    e.try_despawn();
                                                                }
                                                            });
                                                        }
                                                        Some(link) => {
                                                            link.recv.push(payload, Instant::now());
                                                        }
                                                    }
                                                }
                                                Err(_) => {
                                                    error!(
                                                        "Received UDP packet for unknown entity: {}",
                                                        entity
                                                    );
                                                    // this might because the remote entity has disconnected and is trying to reconnect.
                                                    // Remove the entry so that the next packet can be processed
                                                    entry.remove();
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                }
                                Entry::Vacant(vacant) => {
                                    // we are spawning a new entity but the initial packets will be dropped
                                    let mut link = Link::new();
                                    link.recv.push(payload, Instant::now());
                                    commands.command_scope(|mut c| {
                                        let entity = c
                                            .spawn((
                                                LinkOf {
                                                    server: server_entity,
                                                },
                                                link,
                                                Linked,
                                                PeerAddr(address),
                                                UdpLinkOfIO,
                                                // TODO: should we add LocalAddr?
                                            ))
                                            .id();
                                        info!(?entity, ?server_entity, "Received UDP packet from new address {address}, Spawn new LinkOf");
                                        vacant.insert(LinkOfStatus::Spawning(entity));
                                    });
                                    continue;
                                }
                            };
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        // Windows-specific: when a UDP client disconnects, the OS sends an
                        // ICMP "port unreachable" back, which surfaces as ConnectionReset on
                        // the next recv. This is harmless — just skip to the next packet.
                        Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
                        Err(e) => {
                            error!("Error receiving UDP packet: {}", e);
                            break;
                        }
                    }
                }

                // set every spawning to spawned
                server_udp_io.connected_addresses.iter_mut().for_each(|(addr, status)| {
                    if let LinkOfStatus::Spawning(entity) = status {
                        *status = LinkOfStatus::Spawned(*entity);
                    }
                });
            });
    }
}

impl Plugin for ServerUdpPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_observer(Self::link);
        app.add_observer(Self::unlink);
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSystems::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(LinkSystems::Send));
    }
}
