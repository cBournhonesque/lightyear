//! UDP transport for Lightyear links.
//!
//! This crate provides [`UdpIo`], a `std::net::UdpSocket`-backed transport for Lightyear's
//! transport-neutral [`Link`] buffers. UDP is connectionless and packet-oriented: Lightyear's
//! higher-level connection, reliability, replication, and message layers are responsible for any
//! semantics above raw datagram delivery.
//!
//! [`UdpPlugin`] handles single-peer UDP link entities. With the `p2p` or `server` feature enabled,
//! the [`endpoint`] module provides [`endpoint::UdpEndpoint`] and `UdpEndpointPlugin` for a peer
//! socket that creates one child [`Link`] per remote address. With `server`, `server::ServerUdpIo`
//! is the shorthand for an endpoint that is also a server.

// `core::io` is still unstable on the nightly toolchain used to build docs, and this crate already
// requires `std` for `UdpSocket`.
#![allow(clippy::std_instead_of_core)]

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bytes::BufMut;
use lightyear_connection::admission::{DefaultConnectionPlugin, apply_default_ids};
use lightyear_core::buffer_pool::BufferPool;
#[cfg(test)]
use lightyear_core::id::PeerId;
use lightyear_core::id::{LocalId, RemoteId, peer_id_from_socket};
use lightyear_core::time::Instant;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSystems, LinkStart, LinkSystems, Linked, Linking, Unlink, Unlinked,
};
use lightyear_utils::adaptive_for_each_mut;
use tracing::{error, info, trace};

/// Peer-discovery glue for [`lightyear_p2p::Lobby`].
///
/// With the `lobby` feature, this turns [`DialPeer`](lightyear_p2p::DialPeer) into a UDP
/// Link using its native dial context. Peers exchange their opaque routing aliases
/// before admission; a socket address is not their identity.
#[cfg(feature = "lobby")]
pub mod lobby;

/// UDP endpoint: one socket per peer, fanning out to one link per remote address.
///
/// This module is available with the `p2p` or `server` feature.
#[cfg(any(feature = "p2p", feature = "server"))]
pub mod endpoint;

/// Server-side UDP socket support.
///
/// This module is available with the `server` feature. It exposes a server endpoint component that
/// owns one UDP socket and maps remote socket addresses to child Lightyear link entities.
#[cfg(feature = "server")]
pub mod server;

/// Re-exports commonly needed by applications and transport setup code.
pub mod prelude {
    pub use crate::{UdpIo, UdpLocalAddr, UdpPeerAddr};

    /// UDP endpoint prelude: the transport's socket and per-peer link fan-out.
    ///
    /// Available with the `p2p` or `server` feature.
    #[cfg(any(feature = "p2p", feature = "server"))]
    pub mod endpoint {
        pub use crate::endpoint::{UdpEndpoint, UdpEndpointPlugin, UdpLinkOfIO};
    }

    /// Server-side UDP prelude.
    ///
    /// Available with the `server` feature.
    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::endpoint::UdpEndpoint;
        pub use crate::server::ServerUdpIo;
    }
}

/// Maximum UDP payload size used by this transport.
///
/// The value is chosen to avoid common IPv4 fragmentation limits. See
/// <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>.
pub(crate) const MTU: usize = 1472;

const MAX_RETAINED_RECV_BUFFERS: usize = 64;

/// Creates the per-socket receive pool with one buffer allocated during setup.
fn recv_buffer_pool() -> BufferPool {
    let mut pool = BufferPool::new(MTU, MAX_RETAINED_RECV_BUFFERS);
    pool.preallocate(1);
    pool
}

/// The local address reported by a bound UDP socket, including its assigned port.
///
/// This is observed state, not bind configuration. Read it after [`LinkStart`] to obtain the
/// actual address when the transport was configured with port `0`.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct UdpLocalAddr(pub SocketAddr);

/// The remote socket address used to route packets on a UDP link.
///
/// Single-peer sockets publish their configured target when started. Endpoint children receive
/// the datagram source address or the address supplied by the caller when dialing.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct UdpPeerAddr(pub SocketAddr);

/// Single-peer UDP socket transport component.
///
/// Insert this on the entity that owns the Lightyear [`Link`] for a UDP peer. Supply the bind and
/// peer addresses with [`UdpIo::new`]. On [`LinkStart`], the plugin binds the socket and publishes
/// [`UdpLocalAddr`] and [`UdpPeerAddr`] without changing the requested configuration.
///
/// For an endpoint that many peers connect to, use [`endpoint::UdpEndpoint`] instead of one `UdpIo` per
/// remote address.
#[derive(Component)]
#[require(Link)]
pub struct UdpIo {
    /// Requested local bind address. Port `0` lets the OS choose a port.
    pub bind_addr: SocketAddr,
    /// Remote socket address to use when starting the link.
    pub peer_addr: SocketAddr,
    socket: Option<UdpSocket>,
    recv_buffers: BufferPool,
}

impl UdpIo {
    /// Creates a single-peer transport with explicit bind and dial configuration.
    pub fn new(bind: SocketAddr, peer: SocketAddr) -> Self {
        Self {
            bind_addr: bind,
            peer_addr: peer,
            socket: None,
            recv_buffers: recv_buffer_pool(),
        }
    }

    /// Returns receive-buffer pool misses for allocation regression tests.
    #[cfg(feature = "test_utils")]
    pub fn recv_buffer_pool_misses(&self) -> usize {
        self.recv_buffers.misses()
    }
}

/// Bevy plugin that integrates single-peer UDP sockets with Lightyear links.
///
/// The plugin installs:
/// - a [`LinkStart`] observer that binds [`UdpIo`] to its requested address and publishes
///   [`UdpLocalAddr`], [`UdpPeerAddr`], and [`Linked`];
/// - an [`Unlink`] observer that closes the socket;
/// - a receive system in [`LinkReceiveSystems::BufferToLink`] that pushes datagrams into
///   [`Link::recv`];
/// - a send system in [`LinkSystems::Send`] that drains [`Link::send`] to [`UdpPeerAddr`].
///
/// Debug connections are admitted automatically from the bound socket and peer address.
/// Explicit connection backends retain control of authentication and admission.
pub struct UdpPlugin;

impl UdpPlugin {
    fn link(
        trigger: On<LinkStart>,
        mut query: Query<&mut UdpIo, (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        trace!("In LinkStart::UDP trigger");
        if let Ok(mut udp_io) = query.get_mut(trigger.entity) {
            let socket = UdpSocket::bind(udp_io.bind_addr)?;
            let local_addr = socket.local_addr()?;
            info!("UDP socket bound to {}", local_addr);
            socket.set_nonblocking(true)?;
            udp_io.socket = Some(socket);
            let entity = trigger.entity;
            let peer_addr = udp_io.peer_addr;
            let local = LocalId(peer_id_from_socket(b"udp", local_addr));
            let remote = RemoteId(peer_id_from_socket(b"udp", peer_addr));
            commands.queue(move |world: &mut World| {
                world.entity_mut(entity).insert((
                    UdpLocalAddr(local_addr),
                    UdpPeerAddr(peer_addr),
                    Linked,
                ));
                apply_default_ids(world, entity, local, remote);
            });
        }
        Ok(())
    }

    fn unlink(trigger: On<Unlink>, mut query: Query<&mut UdpIo, Without<Unlinked>>) {
        if let Ok(mut udp_io) = query.get_mut(trigger.entity) {
            info!("UDP socket closed");
            udp_io.socket = None;
        }
    }

    fn send(mut query: Query<(&mut Link, &mut UdpIo, &UdpPeerAddr), With<Linked>>) {
        adaptive_for_each_mut!(query).for_each(|(mut link, mut udp_io, remote_addr)| {
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
        adaptive_for_each_mut!(query).for_each(|(mut link, mut udp_io)| {
            // enable split borrows
            let udp_io = &mut *udp_io;
            udp_io.recv_buffers.reclaim_pending();
            loop {
                let mut buffer = udp_io.recv_buffers.take();

                // Check how much actual uninitialized space we have at the end
                let capacity = buffer.capacity();
                let current_len = buffer.len();
                assert_eq!(current_len, 0);
                let available_uninit = capacity - current_len;
                let max_recv_len = core::cmp::min(available_uninit, MTU);

                // We get a raw pointer to the start of the uninitialized region.
                // SAFETY: `take` returns a buffer with at least `MTU` bytes of writable capacity.
                let buf_slice: &mut [u8] = unsafe {
                    let ptr = buffer.as_mut_ptr().add(current_len);
                    core::slice::from_raw_parts_mut(ptr, max_recv_len)
                };
                match udp_io.socket.as_mut().unwrap().recv_from(buf_slice) {
                    Ok((recv_len, _)) => {
                        // Mark the received bytes as initialized
                        // SAFETY: we know that the buffer is large enough to hold the received data.
                        unsafe {
                            buffer.advance_mut(recv_len);
                        }
                        let payload = udp_io.recv_buffers.split_for_handoff(buffer);
                        link.recv.push(payload, Instant::now());
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                        udp_io.recv_buffers.recycle(buffer);
                        return;
                    }
                    // Windows-specific: when the remote end rejects a UDP packet, the OS
                    // raises WSAECONNRESET (10054) on the next recv. This is harmless for
                    // a connectionless UDP socket — just skip to the next receive attempt.
                    Err(ref e) if e.kind() == ErrorKind::ConnectionReset => {
                        udp_io.recv_buffers.recycle(buffer);
                        continue;
                    }
                    Err(e) => {
                        udp_io.recv_buffers.recycle(buffer);
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
        if !app.is_plugin_added::<DefaultConnectionPlugin>() {
            app.add_plugins(DefaultConnectionPlugin);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_client_uses_bound_address_and_disconnect_closes_socket() {
        use lightyear_connection::client::{Client, Connect, Connected, Disconnect, Disconnected};
        use lightyear_connection::network_topology::NetworkingMetadata;
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut app = App::new();
        app.add_plugins(UdpPlugin);
        let entity = app
            .world_mut()
            .spawn((
                Client,
                UdpIo::new("127.0.0.1:0".parse().unwrap(), peer.local_addr().unwrap()),
            ))
            .id();
        app.world_mut().trigger(Connect { entity });
        app.update();
        let local = app.world().get::<UdpLocalAddr>(entity).unwrap().0;
        assert_ne!(local.port(), 0);
        assert_eq!(
            app.world().get::<LocalId>(entity),
            Some(&LocalId(peer_id_from_socket(b"udp", local)))
        );
        assert_eq!(
            app.world().get::<RemoteId>(entity),
            Some(&RemoteId(PeerId::SERVER))
        );
        assert!(app.world().get::<Connected>(entity).is_some());
        app.world_mut()
            .get_mut::<Link>(entity)
            .unwrap()
            .send
            .push(bytes::Bytes::from_static(b"hello"));
        app.update();
        peer.set_read_timeout(Some(core::time::Duration::from_secs(1)))
            .unwrap();
        let mut packet = [0; MTU];
        let (len, source) = peer.recv_from(&mut packet).unwrap();
        assert_eq!(&packet[..len], b"hello");
        assert_eq!(source, local);
        app.world_mut().trigger(Disconnect { entity });
        app.update();
        assert!(app.world().get::<Connected>(entity).is_none());
        assert!(app.world().get::<Linked>(entity).is_none());
        assert!(app.world().get::<Disconnected>(entity).is_some());
        assert!(app.world().get::<UdpIo>(entity).unwrap().socket.is_none());
        assert!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .is_empty()
        );
    }

    #[test]
    fn explicit_backend_and_opt_out_are_not_admitted() {
        use lightyear_connection::admission::NoDefaultConnection;
        use lightyear_connection::client::{Connected, Disconnected};
        #[derive(Component)]
        #[require(NoDefaultConnection)]
        struct Explicit;
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut app = App::new();
        app.add_plugins(UdpPlugin);
        let spawn = |world: &mut World| {
            world
                .spawn(UdpIo::new(
                    "127.0.0.1:0".parse().unwrap(),
                    peer.local_addr().unwrap(),
                ))
                .id()
        };
        let explicit = spawn(app.world_mut());
        let opted_out = spawn(app.world_mut());
        app.world_mut().entity_mut(explicit).insert(Explicit);
        app.world_mut()
            .entity_mut(opted_out)
            .insert(NoDefaultConnection);
        app.world_mut().trigger(LinkStart { entity: explicit });
        app.world_mut().trigger(LinkStart { entity: opted_out });
        app.update();
        for entity in [explicit, opted_out] {
            assert!(app.world().get::<Linked>(entity).is_some());
            assert!(app.world().get::<Connected>(entity).is_none());
            assert!(app.world().get::<LocalId>(entity).is_none());
            assert!(app.world().get::<RemoteId>(entity).is_none());
            assert!(app.world().get::<Disconnected>(entity).is_none());
        }
    }

    #[test]
    fn receive_buffer_pool_reclaims_a_published_datagram_after_drop() {
        let mut pool = recv_buffer_pool();
        let mut buffer = pool.take();
        buffer.extend_from_slice(b"datagram");

        let payload = pool.split_for_handoff(buffer);
        let misses = pool.misses();

        pool.reclaim_pending();
        let in_flight_fallback = pool.take();
        assert_eq!(pool.misses(), misses + 1);
        pool.recycle(in_flight_fallback);

        drop(payload);
        pool.reclaim_pending();
        let misses = pool.misses();
        assert!(pool.take().capacity() >= MTU);
        assert_eq!(pool.misses(), misses);
    }
}
