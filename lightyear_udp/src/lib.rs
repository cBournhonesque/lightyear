//! # Lightyear UDP Transport
//!
//! This crate provides a UDP transport layer for Lightyear.
//! It defines `UdpIo` which uses `std::net::UdpSocket` for sending and receiving raw byte
//! payloads over UDP. This is a common and often preferred transport for real-time games
//! due to its low overhead.
//!
//! The `UdpPlugin` integrates this transport into a Bevy application, managing the
//! lifecycle of UDP sockets and handling the IO operations in conjunction with Lightyear's
//! `Link` component.
//!
//! It also includes server-specific UDP IO handling when the "server" feature is enabled.

use std::{io::ErrorKind, net::UdpSocket};

use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
#[cfg(feature = "metrics")]
use bevy_time::prelude::*;
use bytes::{BufMut, BytesMut};
use lightyear_core::time::Instant;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSet, LinkSet, LinkStart, Linked, Linking, Unlink, Unlinked,
};
use tracing::{error, info, trace};

/// Provides server-specific UDP IO functionalities.
/// This module is only available when the "server" feature is enabled.
#[cfg(feature = "server")]
pub mod server;

/// Commonly used items for UDP transport in Lightyear.
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

/// Component that manages a UDP socket for network communication.
///
/// This component is added to an entity with a `Link` component to enable
/// sending and receiving data over UDP.
/// The user must also add a `LocalAddr` component to specify the local socket address
/// that will be bound.
#[derive(Component)]
#[require(Link)]
// TODO: add LocalAddr using Construct
pub struct UdpIo {
    socket: Option<UdpSocket>,
    buffer: BytesMut,
}

impl Default for UdpIo {
    fn default() -> Self {
        UdpIo {
            socket: None,
            buffer: BytesMut::with_capacity(MTU),
        }
    }
}

/// Errors related to the client connection
#[derive(thiserror::Error, Debug)]
pub enum UdpError {
    #[error("LocalAddr is required to start the UdpIo link")]
    LocalAddrMissing,
}

/// Bevy plugin to integrate UDP-based IO with Lightyear.
///
/// This plugin adds systems to:
/// - Bind UDP sockets when a `LinkStart` event occurs for an entity with `UdpIo` and `Unlinked`.
/// - Close UDP sockets when an `Unlink` event occurs.
/// - Send outgoing packets from the `Link`'s send buffer over UDP.
/// - Receive incoming packets from UDP and push them to the `Link`'s receive buffer.
///
/// It uses the `LinkSet` system sets to order these operations correctly within Bevy's schedule.
pub struct UdpPlugin;

impl UdpPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        mut query: Query<(&mut UdpIo, Option<&LocalAddr>), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        trace!("In LinkStart::UDP trigger");
        if let Ok((mut udp_io, local_addr)) = query.get_mut(trigger.target()) {
            let local_addr = local_addr.ok_or(UdpError::LocalAddrMissing)?.0;
            let socket = UdpSocket::bind(local_addr)?;
            info!("UDP socket bound to {}", local_addr);
            socket.set_nonblocking(true)?;
            udp_io.socket = Some(socket);
            commands.entity(trigger.target()).insert(Linked);
        }
        Ok(())
    }

    fn unlink(trigger: Trigger<Unlink>, mut query: Query<&mut UdpIo, Without<Unlinked>>) {
        if let Ok(mut udp_io) = query.get_mut(trigger.target()) {
            info!("UDP socket closed");
            udp_io.socket = None;
        }
    }

    fn send(
        mut query: Query<(&mut Link, &mut UdpIo, &PeerAddr), With<Linked>>,
        #[cfg(feature = "metrics")]
        time: Res<Time>,
    ) {
        #[cfg(feature = "metrics")]
        metrics::gauge!("udp/send").set(0);
        query
            .par_iter_mut()
            .for_each(|(mut link, mut udp_io, remote_addr)| {
                link.send.drain().for_each(|payload| {
                    // B/s
                    #[cfg(feature = "metrics")]
                    metrics::gauge!("udp/send").increment(payload.len() as f64 / time.delta_secs_f64());
                    udp_io
                        .socket
                        .as_mut()
                        .unwrap()
                        .send_to(payload.as_ref(), remote_addr.0)
                        .inspect_err(|e| error!("Error sending UDP packet: {}", e))
                        .ok();
                });
            })
    }

    fn receive(mut query: Query<(&mut Link, &mut UdpIo), With<Linked>>) {
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
                match udp_io.socket.as_mut().unwrap().recv_from(buf_slice) {
                    Ok((recv_len, _)) => {
                        // Mark the received bytes as initialized
                        // SAFETY: we know that the buffer is large enough to hold the received data.
                        unsafe {
                            udp_io.buffer.advance_mut(recv_len);
                        }
                        let payload = udp_io.buffer.split_to(recv_len);
                        link.recv.push(payload.freeze(), Instant::now());
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => return,
                    Err(e) => {
                        error!("Error receiving UDP packet: {}", e);
                        return;
                    }
                }
            }
        })
    }
}

impl Plugin for UdpPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_observer(Self::link);
        app.add_observer(Self::unlink);
        app.add_systems(
            PreUpdate,
            Self::receive.in_set(LinkReceiveSet::BufferToLink),
        );
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Send));
    }
}
