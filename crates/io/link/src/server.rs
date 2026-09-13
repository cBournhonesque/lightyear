//! The server role: an endpoint that acts as an authority.
//!
//! [`Server`] is a role marker, not the fan-out machinery itself. An entity marked `Server` is an
//! [`Endpoint`] that other peers connect to *as clients* — it is what
//! is what server-side systems and the networking topology classifier query for.
//!
//! Keeping the role separate from the endpoint is what lets the same transport serve a game server
//! and a P2P peer: both own a socket and fan out to one link per remote peer, and only the role
//! marker differs. Transport components that only make sense for a server require this marker, which
//! is also how they get their [`Endpoint`].

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;

use crate::endpoint::Endpoint;

/// Marker component for an endpoint that acts as a server.
///
/// Requires [`Endpoint`]: a role without somewhere to accept links from would be inert.
#[derive(Component, Default, Debug, Reflect)]
#[require(Endpoint)]
pub struct Server;

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;

    #[test]
    fn server_is_an_endpoint() {
        // The role must bring the fan-out target with it; without it a `Server` could not accept
        // links and would silently do nothing.
        let mut app = App::new();
        let server = app.world_mut().spawn(Server).id();
        assert!(app.world().get::<Endpoint>(server).is_some());
    }
}
