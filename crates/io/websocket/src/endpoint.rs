//! WebSocket endpoint transport integration.
//!
//! [`WebSocketEndpoint`] owns the listening socket independently of the server role.
//! Each accepted Aeronet session receives its own Lightyear [`Link`](lightyear_link::Link)
//! through [`LinkOf`].

use crate::WebSocketError;
use aeronet_io::Session;
use aeronet_io::connection::{LocalAddr, PeerAddr};
pub use aeronet_websocket::server::{
    Identity, ServerConfig, WebSocketServer, WebSocketServerClient,
};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_aeronet::endpoint::EndpointAeronetPlugin;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::endpoint::{Endpoint, LinkOf};
use lightyear_link::{Link, LinkStart, Linked, Linking};
use tracing::info;

/// Starts WebSocket endpoints and creates per-peer Lightyear links.
pub struct WebSocketEndpointPlugin;

impl Plugin for WebSocketEndpointPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        if !app.is_plugin_added::<EndpointAeronetPlugin>() {
            app.add_plugins(EndpointAeronetPlugin);
        }
        app.add_plugins(aeronet_websocket::server::WebSocketServerPlugin);

        app.add_observer(Self::link);
        app.add_observer(Self::on_connection);
    }
}

/// Lightyear component for a WebSocket endpoint: one bound address that many peers connect to.
///
/// A [`LocalAddr`] must be present when [`LinkStart`] is triggered; the plugin opens an Aeronet
/// [`WebSocketServer`] using [`config`](Self::config).
///
/// Accepted peers are represented as Lightyear link entities related through [`LinkOf`].
/// Add [`Server`](lightyear_link::server::Server) alongside for the authority role.
#[derive(Component)]
#[require(Endpoint)]
pub struct WebSocketEndpoint {
    /// Aeronet WebSocket server configuration used when opening the socket.
    pub config: ServerConfig,
}

impl WebSocketEndpointPlugin {
    fn link(
        trigger: On<LinkStart>,
        query: Query<
            (Entity, &WebSocketEndpoint, Option<&LocalAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, io, local_addr)) = query.get(trigger.entity) {
            let server_addr = local_addr.ok_or(WebSocketError::LocalAddrMissing)?.0;
            let config = io.config.clone();
            commands.queue(move |world: &mut World| {
                info!("Server WebSocket starting at {}", server_addr);
                let child = world.spawn((AeronetLinkOf(entity), Name::from("WebSocketServer")));
                WebSocketServer::open(config).apply(child);
            });
        }
        Ok(())
    }

    // TODO: should also add on_connecting? Or maybe it's handled automatically
    //  because the connecting entity adds SessionEndpoint? (and lightyear_aeronet handles that)
    fn on_connection(
        trigger: On<Add, Session>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<(&ChildOf, &PeerAddr), With<WebSocketServerClient>>,
        mut commands: Commands,
    ) {
        if let Ok((child_of, peer_addr)) = child_query.get(trigger.entity)
            && let Ok(server_link) = query.get(child_of.parent())
        {
            let link_entity = commands
                .spawn((
                    LinkOf {
                        endpoint: server_link.0,
                    },
                    Link::default(),
                    PeerAddr(peer_addr.0),
                ))
                .id();
            commands
                .entity(trigger.entity)
                .insert((AeronetLinkOf(link_entity), Name::from("WebSocketClientOf")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::{Ipv4Addr, SocketAddr};
    use core::time::Duration;
    use std::thread;

    use lightyear_aeronet::AeronetLink;
    use lightyear_link::{LinkStart, Unlink, UnlinkReason, Unlinked};

    fn server_config(addr: SocketAddr) -> ServerConfig {
        ServerConfig::builder()
            .with_bind_address(addr)
            .with_no_encryption()
    }

    fn spawn_server(app: &mut App, addr: SocketAddr) -> Entity {
        let entity = app
            .world_mut()
            .spawn((
                LocalAddr(addr),
                WebSocketEndpoint {
                    config: server_config(addr),
                },
            ))
            .id();
        app.world_mut().trigger(LinkStart { entity });
        entity
    }

    fn run_app_until(app: &mut App, mut predicate: impl FnMut(&World) -> bool) {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            app.update();
            if predicate(app.world()) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("ran out of time to fulfil predicate");
    }

    #[test]
    fn unlink_releases_server_socket_for_reuse() {
        let mut app = App::new();
        app.add_plugins(WebSocketEndpointPlugin);

        let server = spawn_server(&mut app, SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0));
        run_app_until(&mut app, |world| world.get::<Linked>(server).is_some());
        let local_addr = app.world().get::<LocalAddr>(server).unwrap().0;
        assert_ne!(local_addr.port(), 0);

        app.world_mut().trigger(Unlink {
            entity: server,
            reason: UnlinkReason::UserRequested(Some("test shutdown".to_string())),
        });
        run_app_until(&mut app, |world| {
            world.get::<Unlinked>(server).is_some() && world.get::<AeronetLink>(server).is_none()
        });
        thread::sleep(Duration::from_millis(100));

        let restarted_server = spawn_server(&mut app, local_addr);
        run_app_until(&mut app, |world| {
            world.get::<Linked>(restarted_server).is_some()
                || world.get::<Unlinked>(restarted_server).is_some()
        });

        assert!(
            app.world().get::<Linked>(restarted_server).is_some(),
            "expected restarted server to bind to {local_addr}"
        );
    }
}
