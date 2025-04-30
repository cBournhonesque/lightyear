/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::ecs::query::QueryEntityError;
use bevy::platform::collections::hash_map::Entry;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bytes::{BufMut, BytesMut};
use core::net::SocketAddr;
use lightyear_connection::client::Disconnected;
use lightyear_connection::client_of::{ClientOf, Server};
use lightyear_core::id::PeerId;
use lightyear_link::prelude::{LinkOf, ServerLink};
use lightyear_link::{Link, LinkSet, Linked, Unlinked};
use smallvec::SmallVec;

/// Maximum transmission units; maximum size in bytes of a UDP packet
/// See: <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>
pub(crate) const MTU: usize = 1472;

#[derive(Component)]
#[require(ServerLink)]
pub struct ServerUdpIo {
    local_addr: SocketAddr,
    // TODO: add possibility to set the remote addr
    socket: std::net::UdpSocket,
    buffer: BytesMut,
    connected_addresses: HashMap<SocketAddr, Entity>,
}

impl ServerUdpIo {
    pub fn new(local_addr: SocketAddr) -> std::io::Result<ServerUdpIo> {
        let mut socket = std::net::UdpSocket::bind(local_addr)?;
        info!("Server UDP socket bound to {}", local_addr);
        socket.set_nonblocking(true)?;
        Ok(ServerUdpIo {
            local_addr,
            socket,
            buffer: BytesMut::with_capacity(MTU),
            connected_addresses: HashMap::with_capacity(1),
        })
    }
}

pub struct ServerUdpPlugin;

impl ServerUdpPlugin {
    fn send(
        mut server_query: Query<(&mut ServerUdpIo, &Server)>,
        mut link_query: Query<&mut Link>,
    ) {
        // TODO: parallelize
        server_query
            .iter_mut()
            .for_each(|(mut server_udp_io, server)| {
                server.collection().iter().for_each(|client_entity| {
                    let Some(mut link) = link_query.get_mut(*client_entity).ok() else {
                        error!("Client entity {} not found in link query", client_entity);
                        return;
                    };
                    let Some(remote_addr) = link.remote_addr else {
                        error!("Client entity {} has no remote address", client_entity);
                        return;
                    };
                    link.send.drain().for_each(|send_payload| {
                        server_udp_io
                            .socket
                            .send_to(send_payload.as_ref(), remote_addr)
                            .inspect_err(|e| {
                                error!("Error sending UDP packet to {}: {}", remote_addr, e);
                            })
                            .ok();
                    });
                });
            });
    }

    // TODO:
    //  - server io receives some packets from a new address
    //  - server_io spawns a ClientOf, with Linked
    //     and updates Server->ClientOf mapping to contain the SocketId
    fn receive(
        time: Res<Time<Real>>,
        commands: ParallelCommands,
        mut server_query: Query<(Entity, &mut ServerUdpIo)>,
        mut link_query: Query<(&mut Link)>,
    ) {
        server_query
            .par_iter_mut()
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
                    match server_udp_io.socket.recv_from(buf_slice) {
                        Ok((recv_len, address)) => {
                            // Mark the received bytes as initialized
                            // SAFETY: we know that the buffer is large enough to hold the received data.
                            unsafe {
                                server_udp_io.buffer.advance_mut(recv_len);
                            }
                            let payload = server_udp_io.buffer.split_to(recv_len).freeze();
                            match server_udp_io.connected_addresses.entry(address) {
                                Entry::Occupied(entry) => {
                                    let entity = *entry.get();
                                    match link_query.get_mut(entity) {
                                        Ok(mut link) => {
                                            link.recv.push(payload, time.elapsed());
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
                                Entry::Vacant(vacant) => {
                                    let mut link = Link::new(address, None);
                                    info!("Received UDP packet from new address: {}", address);
                                    link.recv.push(payload, time.elapsed());
                                    commands.command_scope(|mut c| {
                                        let entity = c
                                            .spawn((
                                                LinkOf {
                                                    server: server_entity,
                                                },
                                                ClientOf {
                                                    server: server_entity,
                                                    id: PeerId::Entity,
                                                },
                                                link,
                                                Linked,
                                            ))
                                            .id();
                                        info!(?entity, ?server_entity, "Spawn new ClientOf");
                                        vacant.insert(entity);
                                    });
                                    continue;
                                }
                            };
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                        Err(e) => {
                            error!("Error receiving UDP packet: {}", e);
                            return;
                        }
                    }
                }
            });
    }
}

impl Plugin for ServerUdpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(LinkSet::Send));
    }
}
