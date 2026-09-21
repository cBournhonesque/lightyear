//! WebSocket endpoint transport integration.
//!
//! [`WebSocketEndpoint`] owns the listening socket independently of the server role.
//! Each accepted Aeronet session receives its own Lightyear [`Link`](lightyear_link::Link)
//! through [`LinkOf`].

use crate::{WebSocketLocalAddr, WebSocketPeerAddr};
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
        app.add_observer(Self::on_addresses);
    }
}

/// Lightyear component for a WebSocket endpoint: one bound address that many peers connect to.
///
/// When [`LinkStart`] is triggered, the plugin opens an Aeronet [`WebSocketServer`] using
/// [`config`](Self::config), including its bind address.
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
    fn on_addresses(
        trigger: On<
            Insert,
            (
                LocalAddr,
                PeerAddr,
                AeronetLinkOf,
                WebSocketServer,
                WebSocketServerClient,
            ),
        >,
        query: Query<
            (&AeronetLinkOf, Option<&LocalAddr>, Option<&PeerAddr>),
            Or<(With<WebSocketServer>, With<WebSocketServerClient>)>,
        >,
        mut commands: Commands,
    ) {
        if let Ok((owner, local, peer)) = query.get(trigger.entity) {
            let mut link = commands.entity(owner.0);
            if let Some(local) = local {
                link.insert(WebSocketLocalAddr(local.0));
            }
            if let Some(peer) = peer {
                link.insert(WebSocketPeerAddr(peer.0));
            }
        }
    }

    fn on_linked(
        trigger: On<Add, (Linked, WebSocketLocalAddr, WebSocketPeerAddr)>,
        links: Query<(Entity, &LinkOf, &WebSocketPeerAddr), (With<Linked>, Without<P2P>)>,
        endpoints: Query<&WebSocketLocalAddr, With<WebSocketEndpoint>>,
        mut commands: Commands,
    ) {
        // A bound-address callback can arrive after an accepted session becomes linked.
        let mut admit = |(entity, link_of, peer_addr): (Entity, &LinkOf, &WebSocketPeerAddr)| {
            let Ok(local_addr) = endpoints.get(link_of.endpoint) else {
                return;
            };
            let endpoint = link_of.endpoint;
            let local = LocalId(PeerId::Raw(local_addr.0));
            let remote = RemoteId(PeerId::Raw(peer_addr.0));
            commands.queue(move |world: &mut World| {
                #[cfg(feature = "lobby")]
                if world.get::<crate::lobby::LobbyEndpoint>(endpoint).is_some() {
                    return;
                }
                let _ = endpoint;
                apply_default_ids(world, entity, local, remote);
            });
        };
        if endpoints.contains(trigger.entity) {
            for link in &links {
                if link.1.endpoint == trigger.entity {
                    admit(link);
                }
            }
        } else if let Ok(link) = links.get(trigger.entity) {
            admit(link);
        }
    }

    fn link(
        trigger: On<LinkStart>,
        query: Query<(Entity, &WebSocketEndpoint), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, io)) = query.get(trigger.entity) {
            let config = io.config.clone();
            commands.queue(move |world: &mut World| {
                info!("Server WebSocket starting");
                let child = world.spawn((AeronetLinkOf(entity), Name::from("WebSocketServer")));
                WebSocketServer::open(config).apply(child);
            });
        }
        Ok(())
    }

    fn on_connection(
        trigger: On<Add, (Session, WebSocketServerClient)>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<
            &ChildOf,
            (
                With<WebSocketServerClient>,
                With<Session>,
                Without<AeronetLinkOf>,
            ),
        >,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = child_query.get(trigger.entity)
            && let Ok(server_link) = query.get(child_of.parent())
        {
            let link_entity = commands
                .spawn((
                    LinkOf {
                        endpoint: server_link.0,
                    },
                    Link::default(),
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
    #[test]
    fn accepted_session_waits_for_bound_and_peer_addresses() {
        use lightyear_connection::client::Connected;

        let mut app = App::new();
        app.add_plugins(WebSocketEndpointPlugin);
        let local = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 4100);
        let peer = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 4101);
        let endpoint = app
            .world_mut()
            .spawn(WebSocketEndpoint {
                config: server_config(local),
            })
            .id();
        let link = app
            .world_mut()
            .spawn((LinkOf { endpoint }, Link::default(), Linked))
            .id();
        assert!(app.world().get::<Connected>(link).is_none());
        app.world_mut()
            .entity_mut(link)
            .insert(WebSocketPeerAddr(peer));
        assert!(app.world().get::<Connected>(link).is_none());
        app.world_mut()
            .entity_mut(endpoint)
            .insert(WebSocketLocalAddr(local));
        assert!(app.world().get::<Connected>(link).is_some());
        assert_eq!(
            app.world().get::<RemoteId>(link).unwrap().0,
            PeerId::Raw(peer)
        );
    }

    #[cfg(feature = "client")]
    #[test]
    fn local_sessions_respect_admission_ownership_and_disconnect() {
        use crate::client::{WebSocketClientIo, WebSocketClientPlugin};
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
                    WebSocketEndpoint {
                        config: server_config(address).with_handshake_handler(
                            aeronet_websocket::server::HandshakeHandler::new(
                                |request, response| {
                                    assert_eq!(
                                        request.uri().path_and_query().unwrap().as_str(),
                                        "/session?transport=ws"
                                    );
                                    assert!(
                                        request.headers()["host"]
                                            .to_str()
                                            .unwrap()
                                            .starts_with("localhost:")
                                    );
                                    Ok(response)
                                },
                            ),
                        ),
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
            let address = app.world().get::<WebSocketLocalAddr>(server).unwrap().0;
            let client = client_app
                .world_mut()
                .spawn(WebSocketClientIo::from_url(
                    Default::default(),
                    format!("ws://localhost:{}/session?transport=ws", address.port()),
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
            assert_eq!(
                client_app
                    .world()
                    .get::<WebSocketPeerAddr>(client)
                    .unwrap()
                    .0,
                address
            );
            for entity in [server, accepted] {
                assert!(app.world().get::<LocalAddr>(entity).is_none());
                assert!(app.world().get::<PeerAddr>(entity).is_none());
            }
            assert!(client_app.world().get::<LocalAddr>(client).is_none());
            assert!(client_app.world().get::<PeerAddr>(client).is_none());
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
                    PeerId::Raw(
                        client_app
                            .world()
                            .get::<WebSocketLocalAddr>(client)
                            .unwrap()
                            .0
                    ),
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
            .spawn(WebSocketEndpoint {
                config: server_config(addr),
            })
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
        let local_addr = app.world().get::<WebSocketLocalAddr>(server).unwrap().0;
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
