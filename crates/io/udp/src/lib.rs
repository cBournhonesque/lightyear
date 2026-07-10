//! UDP transport for Lightyear links.
//!
//! This crate provides [`UdpIo`], a `std::net::UdpSocket`-backed transport for Lightyear's
//! transport-neutral [`Link`] buffers. UDP is connectionless and packet-oriented: Lightyear's
//! higher-level connection, reliability, replication, and message layers are responsible for any
//! semantics above raw datagram delivery.
//!
//! [`UdpPlugin`] handles single-peer UDP link entities. With the `server` feature enabled, the
//! [`server`] module provides [`server::ServerUdpIo`] and `ServerUdpPlugin` for a listening server
//! socket that creates one child [`Link`] per remote address.

use std::{io::ErrorKind, net::UdpSocket};

use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bytes::{BufMut, BytesMut};
use lightyear_core::time::Instant;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSystems, LinkStart, LinkSystems, Linked, Linking, Unlink, Unlinked,
};
use tracing::{error, info, trace};

/// Server-side UDP socket support.
///
/// This module is available with the `server` feature. It exposes a server endpoint component that
/// owns one UDP socket and maps remote socket addresses to child Lightyear link entities.
#[cfg(feature = "server")]
pub mod server;

/// Re-exports commonly needed by applications and transport setup code.
pub mod prelude {
    pub use crate::UdpIo;

    /// Server-side UDP prelude.
    ///
    /// Available with the `server` feature.
    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::ServerUdpIo;
    }
}

/// Maximum UDP payload size used by this transport.
///
/// The value is chosen to avoid common IPv4 fragmentation limits. See
/// <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>.
pub(crate) const MTU: usize = 1472;

/// Single-peer UDP socket transport component.
///
/// Insert this on the entity that owns the Lightyear [`Link`] for a UDP peer. A [`LocalAddr`] must
/// be present before [`LinkStart`] is triggered so the plugin can bind the socket, and [`PeerAddr`]
/// must be present while linked so outgoing packets know their destination.
///
/// For listening servers with many clients, use [`server::ServerUdpIo`] instead of one `UdpIo` per
/// remote address.
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

/// Errors produced while starting UDP transport entities.
#[derive(thiserror::Error, Debug)]
pub enum UdpError {
    /// The entity did not have a [`LocalAddr`] when [`LinkStart`] was processed.
    #[error("LocalAddr is required to start the UdpIo link")]
    LocalAddrMissing,
}

/// Bevy plugin that integrates single-peer UDP sockets with Lightyear links.
///
/// The plugin installs:
/// - a [`LinkStart`] observer that binds [`UdpIo`] to [`LocalAddr`] and marks the entity
///   [`Linked`];
/// - an [`Unlink`] observer that closes the socket;
/// - a receive system in [`LinkReceiveSystems::BufferToLink`] that pushes datagrams into
///   [`Link::recv`];
/// - a send system in [`LinkSystems::Send`] that drains [`Link::send`] to [`PeerAddr`].
///
/// This is a raw datagram transport. Use Lightyear connection plugins above it when you need
/// connection state, authentication, or session management.
pub struct UdpPlugin;

impl UdpPlugin {
    fn link(
        trigger: On<LinkStart>,
        mut query: Query<(&mut UdpIo, Option<&LocalAddr>), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        trace!("In LinkStart::UDP trigger");
        if let Ok((mut udp_io, local_addr)) = query.get_mut(trigger.entity) {
            let local_addr = local_addr.ok_or(UdpError::LocalAddrMissing)?.0;
            let socket = UdpSocket::bind(local_addr)?;
            info!("UDP socket bound to {}", local_addr);
            socket.set_nonblocking(true)?;
            udp_io.socket = Some(socket);
            commands.entity(trigger.entity).insert(Linked);
        }
        Ok(())
    }

    fn unlink(trigger: On<Unlink>, mut query: Query<&mut UdpIo, Without<Unlinked>>) {
        if let Ok(mut udp_io) = query.get_mut(trigger.entity) {
            info!("UDP socket closed");
            udp_io.socket = None;
        }
    }

    fn send(mut query: Query<(&mut Link, &mut UdpIo, &PeerAddr), With<Linked>>) {
        query
            .par_iter_mut()
            .for_each(|(mut link, mut udp_io, remote_addr)| {
                link.send.drain().for_each(|payload| {
                    // B/s
                    #[cfg(feature = "metrics")]
                    metrics::gauge!("udp/send").increment(payload.len() as f64);
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
                    // Windows-specific: when the remote end rejects a UDP packet, the OS
                    // raises WSAECONNRESET (10054) on the next recv. This is harmless for
                    // a connectionless UDP socket — just skip to the next receive attempt.
                    Err(ref e) if e.kind() == ErrorKind::ConnectionReset => continue,
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
            Self::receive.in_set(LinkReceiveSystems::BufferToLink),
        );
        app.add_systems(PostUpdate, Self::send.in_set(LinkSystems::Send));
    }
}
