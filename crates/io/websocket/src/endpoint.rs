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
use lightyear_connection::admission::{DefaultConnectionPlugin, apply_default_ids};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
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
        if !app.is_plugin_added::<DefaultConnectionPlugin>() {
            app.add_plugins(DefaultConnectionPlugin);
        }
        app.add_plugins(aeronet_websocket::server::WebSocketServerPlugin);

        app.add_observer(Self::link);
        app.add_observer(Self::on_connection);
        app.add_observer(Self::on_linked);
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
    fn on_linked(
        trigger: On<Add, Linked>,
        links: Query<(&LinkOf, &PeerAddr), Without<P2P>>,
        endpoints: Query<&LocalAddr, With<WebSocketEndpoint>>,
        mut commands: Commands,
    ) {
        let Ok((link_of, peer_addr)) = links.get(trigger.entity) else {
            return;
        };
        let Ok(local_addr) = endpoints.get(link_of.endpoint) else {
            return;
        };
        let entity = trigger.entity;
        let endpoint = link_of.endpoint;
        let local = LocalId(PeerId::Raw(local_addr.0));
        let remote = RemoteId(PeerId::Raw(peer_addr.0));
        commands.queue(move |world: &mut World| {
            // Lobby links are admitted only after their P2P IDs and roles have been installed.
            #[cfg(feature = "lobby")]
            if world.get::<crate::lobby::LobbyEndpoint>(endpoint).is_some() {
                return;
            }
            let _ = endpoint;
            apply_default_ids(world, entity, local, remote);
        });
    }

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
    #[cfg(feature = "client")]
    #[test]
    fn local_sessions_respect_admission_ownership_and_disconnect() {
        use crate::client::{WebSocketClientIo, WebSocketClientPlugin, WebSocketScheme};
        use lightyear_connection::admission::NoDefaultConnection;
        use lightyear_connection::client::{Connected, Disconnect};
        use lightyear_connection::client_of::ClientOf;
        use lightyear_connection::network_topology::NetworkingMetadata;
        use lightyear_link::server::Server;
        use lightyear_netcode::auth::Authentication;

        fn run_apps_until(
            server: &mut App,
            client: &mut App,
            predicate: impl Fn(&World, &World) -> bool,
        ) {
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(2) {
                server.update();
                client.update();
                if predicate(server.world(), client.world()) {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            panic!("ran out of time to fulfil predicate");
        }

        // Exercise bare default admission, explicit opt-out, and real netcode ownership on
        // both dialed and accepted sessions. Netcode does not run its handshake in this test.
        for mode in 0..3 {
            let mut app = App::new();
            app.add_plugins(WebSocketEndpointPlugin);
            let mut client_app = App::new();
            client_app.add_plugins(WebSocketClientPlugin);
            let address = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
            let server = app
                .world_mut()
                .spawn((
                    Server,
                    LocalAddr(address),
                    WebSocketEndpoint {
                        config: server_config(address),
                    },
                ))
                .id();
            if mode == 1 {
                app.world_mut()
                    .entity_mut(server)
                    .insert(NoDefaultConnection);
            } else if mode == 2 {
                app.world_mut()
                    .entity_mut(server)
                    .insert(lightyear_netcode::NetcodeServer::new(
                        lightyear_netcode::server_plugin::NetcodeConfig::default(),
                    ));
            }
            app.world_mut().trigger(LinkStart { entity: server });
            run_app_until(&mut app, |world| world.get::<Linked>(server).is_some());
            let address = app.world().get::<LocalAddr>(server).unwrap().0;
            let client = client_app
                .world_mut()
                .spawn((
                    PeerAddr(address),
                    WebSocketClientIo::from_addr(Default::default(), WebSocketScheme::Plain),
                ))
                .id();
            if mode == 1 {
                client_app
                    .world_mut()
                    .entity_mut(client)
                    .insert(NoDefaultConnection);
            } else if mode == 2 {
                client_app.world_mut().entity_mut(client).insert(
                    lightyear_netcode::NetcodeClient::new(
                        Authentication::Manual {
                            server_addr: address,
                            client_id: 7,
                            private_key: [0; lightyear_netcode::PRIVATE_KEY_BYTES],
                            protocol_id: 1,
                        },
                        lightyear_netcode::client_plugin::NetcodeConfig::default(),
                    )
                    .unwrap(),
                );
            }
            client_app.world_mut().trigger(LinkStart { entity: client });
            run_apps_until(&mut app, &mut client_app, |server_world, client_world| {
                client_world.get::<Linked>(client).is_some()
                    && server_world.iter_entities().any(|entity| {
                        entity
                            .get::<LinkOf>()
                            .is_some_and(|link| link.endpoint == server)
                            && entity.contains::<Linked>()
                    })
            });
            let accepted = app
                .world()
                .iter_entities()
                .find(|entity| {
                    entity
                        .get::<LinkOf>()
                        .is_some_and(|link| link.endpoint == server)
                })
                .unwrap()
                .id();
            if mode == 0 {
                assert!(client_app.world().get::<Connected>(client).is_some());
                assert!(app.world().get::<Connected>(accepted).is_some());
                assert!(app.world().get::<ClientOf>(accepted).is_some());
                assert_eq!(
                    client_app.world().get::<RemoteId>(client).unwrap().0,
                    PeerId::Server
                );
                assert_eq!(
                    app.world().get::<RemoteId>(accepted).unwrap().0,
                    PeerId::Raw(client_app.world().get::<LocalAddr>(client).unwrap().0),
                );
                client_app
                    .world_mut()
                    .trigger(Disconnect { entity: client });
            } else {
                assert!(client_app.world().get::<Connected>(client).is_none());
                assert!(app.world().get::<Connected>(accepted).is_none());
                client_app.world_mut().trigger(Unlink {
                    entity: client,
                    reason: UnlinkReason::UserRequested(None),
                });
            }
            run_apps_until(&mut app, &mut client_app, |server_world, client_world| {
                client_world.get::<Unlinked>(client).is_some()
                    && (server_world.get_entity(accepted).is_err()
                        || server_world.get::<Unlinked>(accepted).is_some())
            });
            assert!(
                !app.world()
                    .resource::<NetworkingMetadata>()
                    .peer_map
                    .values()
                    .any(|owner| *owner == accepted)
            );
            assert!(
                client_app
                    .world()
                    .resource::<NetworkingMetadata>()
                    .peer_map
                    .is_empty()
            );
            app.world_mut().trigger(Unlink {
                entity: server,
                reason: UnlinkReason::UserRequested(None),
            });
            run_app_until(&mut app, |world| world.get::<Unlinked>(server).is_some());
            assert!(
                app.world()
                    .resource::<NetworkingMetadata>()
                    .peer_map
                    .is_empty()
            );
        }
    }

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
