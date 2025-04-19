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
use bytes::{BufMut, BytesMut};
use core::net::SocketAddr;
use lightyear_link::{Link, LinkSet, Linked};
use std::sync::Mutex;

#[cfg(feature = "server")]
pub mod server;

pub mod prelude {
    pub use crate::UdpIo;

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::ServerUdpIo;
    }
}

/// Maximum transmission units; maximum size in bytes of a UDP packet
/// See: <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>
pub(crate) const MTU: usize = 1472;

#[derive(Component)]
#[require(Link)]
// There is no linking phase
#[require(Linked)]
pub struct UdpIo {
    local_addr: SocketAddr,
    // TODO: add possibility to set the remote addr
    socket: std::net::UdpSocket,
    buffer: BytesMut,
}

impl UdpIo {
    pub fn new(local_addr: SocketAddr) -> std::io::Result<UdpIo> {
        let mut socket = std::net::UdpSocket::bind(local_addr)?;
        info!("UDP socket bound to {}", local_addr);
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
                link.send.drain().for_each(|payload| {
                    info!("Sending UDP packet of size {:?} to {}", payload.len(), remote_addr);
                    // TODO: how do we get the link address?
                    //   Maybe Link has multiple states?
                    // TODO: we don't want to short-circuit on error
                    udp_io.socket.send_to(payload.as_ref(), remote_addr).inspect_err(|e| error!("Error sending UDP packet: {}", e)).ok();
                });
            }
        })
    }

    fn receive(
        time: Res<Time<Real>>,
        mut query: Query<(&mut Link, &mut UdpIo)>
    ) {
        query.par_iter_mut().for_each(|(mut link, mut udp_io)| {
            // enable split borrows
            let udp_io = &mut *udp_io;
            loop {
                // TODO: this might cause Copy-on-Writes and re-allocations if we receive more than MTU bytes
                //  in one frame. Solutions:
                // 1. use a bump allocator to temporarily store the messages before deserializing them
                // 2. track how often we need to reclaim memory, or track how many bytes we receive each frame,
                //    and increase the size of the buffer accordingly according to the average/median of the last
                //    few seconds

                // reserve additional space in the buffer
                // this tries to reclaim space at the start of the buffer if possible
                udp_io.buffer.reserve(MTU);

                // Check how much actual uninitialized space we have at the end
                let capacity = udp_io.buffer.capacity();
                let current_len = udp_io.buffer.len();
                assert_eq!(current_len, 0);
                let available_uninit = capacity - current_len;
                let max_recv_len = core::cmp::min(available_uninit, MTU);

                // We get a raw pointer to the start of the uninitialized region.
                // SAFETY: we know we have enough space to receive the data because we just reserved it
                let buf_slice: &mut [u8] = unsafe {
                    let ptr = udp_io.buffer.as_mut_ptr().add(current_len);
                    core::slice::from_raw_parts_mut(ptr, max_recv_len)
                };
                match udp_io.socket.recv_from(buf_slice) {
                    Ok((recv_len, _)) => {
                        // Mark the received bytes as initialized
                        // SAFETY: we know that the buffer is large enough to hold the received data.
                        unsafe {
                            udp_io.buffer.advance_mut(recv_len);
                        }
                        let payload = udp_io.buffer.split_to(recv_len);
                        link.recv.push(payload.freeze(), time.elapsed());
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



