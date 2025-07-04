/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/

extern crate alloc;

use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::error::Result;
use bevy_ecs::observer::Trigger;
use bevy_ecs::query::{With, Without};
use bevy_ecs::relationship::RelationshipTarget;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_ecs::system::{Commands, ParallelCommands, Query};
use tracing::{debug, error, info};

use crate::UdpError;
use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_platform::collections::{HashMap, hash_map::Entry};
use bytes::{BufMut, BytesMut};
use core::net::SocketAddr;
use lightyear_core::time::Instant;
use lightyear_link::prelude::{LinkOf, Server};
use lightyear_link::{Link, LinkPlugin, LinkSet, LinkStart, Linked, Linking, Unlink, Unlinked};

/// Maximum transmission units; maximum size in bytes of a UDP packet
/// See: <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>
pub(crate) const MTU: usize = 1472;

/// Component to start a UdpServer.
///
/// The [`LocalAddr`] component is required to specify the local SocketAddr to bind.
#[derive(Component)]
#[require(Server)]
pub struct ServerUdpIo {
    socket: Option<std::net::UdpSocket>,
    buffer: BytesMut,
    connected_addresses: HashMap<SocketAddr, LinkOfStatus>,
}

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

pub struct ServerUdpPlugin;

impl ServerUdpPlugin {
    // TODO: we don't want this system to panic on error
    fn link(
        trigger: Trigger<LinkStart>,
        mut query: Query<
            (&mut ServerUdpIo, Option<&LocalAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((mut udp_io, local_addr)) = query.get_mut(trigger.target()) {
            let local_addr = local_addr.ok_or(UdpError::LocalAddrMissing)?.0;
            info!("Server UDP socket bound to {}", local_addr);
            let socket = std::net::UdpSocket::bind(local_addr)?;
            socket.set_nonblocking(true)?;
            udp_io.socket = Some(socket);
            commands.entity(trigger.target()).insert(Linked);
        }
        Ok(())
    }

    fn unlink(trigger: Trigger<Unlink>, mut query: Query<&mut ServerUdpIo, Without<Unlinked>>) {
        if let Ok(mut udp_io) = query.get_mut(trigger.target()) {
            info!("Server UDP socket closed");
            udp_io.socket = None;
        }
    }

    fn send(
        mut server_query: Query<(&mut ServerUdpIo, &Server), With<Linked>>,
        mut link_query: Query<(&mut Link, &PeerAddr)>,
    ) {
        // TODO: parallelize
        server_query
            .iter_mut()
            .for_each(|(mut server_udp_io, server)| {
                server.collection().iter().for_each(|client_entity| {
                    let Some((mut link, remote_addr)) = link_query.get_mut(*client_entity).ok()
                    else {
                        error!("Client entity {} not found in link query", client_entity);
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
                                    let mut link = Link::new(None);
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
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(LinkSet::Send));
    }
}
