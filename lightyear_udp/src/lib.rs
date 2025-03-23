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

/// Maximum transmission units; maximum size in bytes of a UDP packet
/// See: <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>
pub(crate) const MTU: usize = 1472;

#[derive(Component)]
pub struct UdpIo {
    local_addr: SocketAddr,
    sender: UdpSocketBuffer,
    receiver: UdpSocketBuffer,
}

impl UdpIo {
    pub fn new(local_addr: SocketAddr) -> std::io::Result<UdpIo> {
        let udp_socket = std::net::UdpSocket::bind(local_addr)?;
        let local_addr = udp_socket.local_addr()?;
        let socket = Arc::new(Mutex::new(udp_socket));
        socket.as_ref().lock().unwrap().set_nonblocking(true)?;
        let sender = UdpSocketBuffer {
            socket: socket.clone(),
            buffer: BytesMut::with_capacity(MTU),
        };
        let receiver = sender.clone();
        Ok(UdpIo {
            local_addr,
            sender,
            receiver,
        })
    }
}

#[derive(Clone)]
pub struct UdpSocketBuffer {
    /// The underlying UDP Socket. This is wrapped in an Arc<Mutex<>> so that it
    /// can be shared between threads
    socket: Arc<Mutex<std::net::UdpSocket>>,
    buffer: BytesMut,
}

pub struct UdpPlugin;

impl UdpPlugin {
    fn send(
        mut query: Query<(&mut Link, &mut UdpIo)>
    ) -> Result {
        // TODO: parallelize
        query.iter_mut().try_for_each(|(mut link, mut udp_io)| {
            // TODO: actually we don't need Arc<Mutex> because the scheduler
            //  guarantees that we don't access the same socket at the same time
            let socket = udp_io.sender.socket.lock().unwrap();
            link.send.drain(..).try_for_each(|payload| {
                // TODO: how do we get the link address?
                //   Maybe Link has multiple states?
                // TODO: we don't want to short-circuit on error
                socket.send_to(payload.as_ref(), link.address)?;
                Ok(())
            })?;
            Ok(())
        })
    }

    fn receive(
        mut query: Query<(&mut Link, &mut UdpIo)>
    ) -> Result {
        // TODO: parallelize
        query.iter_mut().try_for_each(|(mut link, mut udp_io)| {
            // TODO: actually we don't need Arc<Mutex> because the scheduler
            //  guarantees that we don't access the same socket at the same time
            // enable split borrows
            let udp_io = &mut *udp_io;
            let socket = udp_io.receiver.socket.lock().unwrap();
            loop {
                // reserve additional space in the buffer
                // this tries to reclaim space at the start of the buffer if possible
                udp_io.receiver.buffer.reserve(MTU);
                match socket.recv_from(&mut udp_io.receiver.buffer) {
                    Ok((recv_len, address)) => {
                        let payload = udp_io.receiver.buffer.split_to(recv_len);
                        link.recv.push(payload.freeze());
                        return Ok(())
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Nothing to receive on the socket
                        return Ok(())
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        })
    }
}
impl Plugin for UdpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Receive));
    }
}



