/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use alloc::sync::Arc;
use bevy::prelude::*;
use bytes::BytesMut;
use core::net::SocketAddr;
use lightyear_link::{Link, LinkSet};
use std::sync::Mutex;

#[cfg(feature = "server")]
pub mod server;

/// Maximum transmission units; maximum size in bytes of a UDP packet
/// See: <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>
pub(crate) const MTU: usize = 1472;

#[derive(Component)]
pub struct UdpIo {
    local_addr: SocketAddr,
    // TODO: add possibility to set the remote addr
    socket: std::net::UdpSocket,
    buffer: BytesMut,
}

impl UdpIo {
    pub fn new(local_addr: SocketAddr) -> std::io::Result<UdpIo> {
        let mut socket = std::net::UdpSocket::bind(local_addr)?;
        socket.set_nonblocking(true)?;
        Ok(UdpIo {
            local_addr,
            socket,
            buffer: BytesMut::with_capacity(MTU),
        })
    }
}

pub struct UdpPlugin;

impl UdpPlugin {
    fn send(
        mut query: Query<(&mut Link, &mut UdpIo)>
    ) {
        query.par_iter_mut().for_each(|(mut link, mut udp_io)| {
            if let Some(remote_addr) = link.remote_addr {
                link.send.drain(..).for_each(|payload| {
                    // TODO: how do we get the link address?
                    //   Maybe Link has multiple states?
                    // TODO: we don't want to short-circuit on error
                    udp_io.socket.send_to(payload.as_ref(), remote_addr).inspect_err(|e| error!("Error sending UDP packet: {}", e)).ok();
                });
            }
        })
    }

    fn receive(
        mut query: Query<(&mut Link, &mut UdpIo)>
    ) {
        query.par_iter_mut().for_each(|(mut link, mut udp_io)| {
            // TODO: actually we don't need Arc<Mutex> because the scheduler
            //  guarantees that we don't access the same socket at the same time
            // enable split borrows
            let udp_io = &mut *udp_io;
            loop {
                // reserve additional space in the buffer
                // this tries to reclaim space at the start of the buffer if possible
                udp_io.buffer.reserve(MTU);
                match udp_io.socket.recv_from(&mut udp_io.buffer) {
                    Ok((recv_len, address)) => {
                        let payload = udp_io.buffer.split_to(recv_len);
                        link.recv.push(payload.freeze());
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

impl Plugin for UdpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Send));
    }
}



