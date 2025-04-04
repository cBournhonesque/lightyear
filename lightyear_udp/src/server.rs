/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::ecs::query::QueryEntityError;
use bevy::prelude::*;
use bytes::BytesMut;
use core::net::SocketAddr;
use lightyear_connection::client::Disconnected;
use lightyear_connection::client_of::{ClientOf, Server};
use lightyear_connection::id::PeerId;
use lightyear_link::{Link, LinkSet, Linked, Unlinked};

/// Maximum transmission units; maximum size in bytes of a UDP packet
/// See: <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>
pub(crate) const MTU: usize = 1472;

#[derive(Component)]
pub struct ServerUdpIo {
    local_addr: SocketAddr,
    // TODO: add possibility to set the remote addr
    socket: std::net::UdpSocket,
    buffer: BytesMut,
}

impl ServerUdpIo {
    pub fn new(local_addr: SocketAddr) -> std::io::Result<ServerUdpIo> {
        let mut socket = std::net::UdpSocket::bind(local_addr)?;
        socket.set_nonblocking(true)?;
        Ok(ServerUdpIo {
            local_addr,
            socket,
            buffer:  BytesMut::with_capacity(MTU)
        })
    }
}


pub struct ServerUdpPlugin;

impl ServerUdpPlugin {
    fn send(
        mut server_query: Query<(&mut ServerUdpIo, &Server)>,
        mut link_query: Query<&mut Link, Without<Unlinked>>
    ) {
        server_query.par_iter_mut().for_each(|(mut server_udp_io, server)| {
            server.collection().iter().for_each(|client_entity| {
                let Some(mut link) = link_query.get_mut(*client_entity).ok() else {
                    error!("Client entity {} not found in link query", client_entity);
                    return
                };
                let Some(remote_addr) = link.remote_addr else {
                    error!("Client entity {} has no remote address", client_entity);
                    return
                };
                link.send.drain(..).for_each(|send_payload| {
                    server_udp_io.socket.send_to(send_payload.as_ref(), remote_addr).inspect_err(|e| {
                        error!("Error sending UDP packet to {}: {}", remote_addr, e);
                    }).ok();
                });
            });
        });
    }

    // TODO:
    //  - server io receives some packets from a new address
    //  - server_io spawns a ClientOf, with Linked
    //     and updates Server->ClientOf mapping to contain the SocketId
    fn receive(
        commands: ParallelCommands,
        mut server_query: Query<(Entity, &mut ServerUdpIo, &Server)>,
        mut link_query: Query<(&mut Link)>
    ) {
        server_query.par_iter_mut().for_each(|(server_entity, mut server_udp_io, server)| {
            // enable split borrows
            let server_udp_io = &mut *server_udp_io;
            loop {
                // reserve additional space in the buffer
                // this tries to reclaim space at the start of the buffer if possible
                server_udp_io.buffer.reserve(crate::MTU);
                match server_udp_io.socket.recv_from(&mut server_udp_io.buffer) {
                    Ok((recv_len, address)) => {
                        let payload = server_udp_io.buffer.split_to(recv_len);
                        let peer_id = PeerId::Entity;
                        let Some(entity) = server.get_client(peer_id) else {
                            info!("Received UDP packet from new address: {}", address);
                            let mut link = Link::new(server_udp_io.local_addr);
                            link.recv.push(payload.freeze());
                            commands.command_scope(|mut c| {
                                c.spawn((ClientOf {
                                    server: server_entity,
                                    id: peer_id,
                                }, link, Linked));
                            });
                            continue
                        };
                        match link_query.get_mut(entity) {
                            Ok(link) => {
                                link.recv.push(payload.freeze());
                            }
                            Err(_) => {
                                error!("Received UDP packet for unknown entity: {}", entity);
                                continue
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        return
                    }
                    Err(e) => {
                        error!("Error receiving UDP packet: {}", e);
                        return
                    },
                }
            }
        })
    }
}
impl Plugin for ServerUdpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Send));
    }
}



