//! Server-role sugar for the UDP endpoint.
//!
//! [`ServerUdpIo`](crate::server::ServerUdpIo) is the shorthand for "this entity is a UDP
//! **server**": it inserts the transport's
//! [`UdpEndpoint`](crate::endpoint::UdpEndpoint) together with Lightyear's
//! [`Server`](lightyear_link::server::Server) role marker. Use it in the same slots where a
//! server used to be declared; reach for [`UdpEndpoint`](crate::endpoint::UdpEndpoint) directly
//! when the entity is not a server,
//! such as a peer in a P2P session.
//!
//! The endpoint machinery itself — the socket, the per-peer fan-out, and the plugin — lives in
//! [`crate::endpoint`], because none of it is server-specific.

use bevy_ecs::prelude::*;
use lightyear_link::server::Server;

use crate::endpoint::UdpEndpoint;

/// Marks an entity as a UDP server: requires both [`UdpEndpoint`] and [`Server`].
///
/// A [`LocalAddr`](aeronet_io::connection::LocalAddr) is still required before
/// [`LinkStart`](lightyear_link::LinkStart) is triggered, exactly as for a bare [`UdpEndpoint`].
#[derive(Component, Default, Debug)]
#[require(UdpEndpoint, Server)]
pub struct ServerUdpIo;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::UdpEndpoint;
    use bevy_app::App;
    use lightyear_link::prelude::Endpoint;

    #[test]
    fn server_udp_io_is_an_endpoint_with_the_server_role() {
        // The shorthand has to bring up both halves, or a declared server would be missing either its
        // socket or its role.
        let mut app = App::new();
        let server = app.world_mut().spawn(ServerUdpIo).id();
        let world = app.world();
        assert!(world.get::<UdpEndpoint>(server).is_some());
        assert!(world.get::<Server>(server).is_some());
        assert!(world.get::<Endpoint>(server).is_some(), "from the role");
    }
}
