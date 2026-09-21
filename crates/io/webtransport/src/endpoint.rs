//! WebTransport endpoint transport integration.
//!
//! [`WebTransportEndpoint`] owns the listening socket independently of the server role.
//! Each accepted Aeronet session receives its own Lightyear [`Link`](lightyear_link::Link)
//! through [`LinkOf`].

use crate::{WebTransportLocalAddr, WebTransportPeerAddr};
use aeronet_io::Session;
use aeronet_io::connection::{LocalAddr, PeerAddr};
use aeronet_webtransport::server::{
    ServerConfig, SessionRequest, SessionResponse, WebTransportServer, WebTransportServerClient,
};
use aeronet_webtransport::wtransport::Identity;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use core::net::SocketAddr;
use core::time::Duration;
use lightyear_aeronet::endpoint::EndpointAeronetPlugin;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_connection::admission::{DefaultConnectionPlugin, apply_default_ids};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::endpoint::{Endpoint, LinkOf};
use lightyear_link::{Link, LinkStart, Linked, Linking};
use tracing::info;

/// Starts WebTransport endpoints, accepts session requests and creates per-peer Lightyear links.
pub struct WebTransportEndpointPlugin;

impl Plugin for WebTransportEndpointPlugin {
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
        app.add_plugins(aeronet_webtransport::server::WebTransportServerPlugin);

        app.add_observer(Self::link);
        app.add_observer(Self::on_session_request);
        app.add_observer(Self::on_connection);
        app.add_observer(Self::on_linked);
        app.add_observer(Self::on_addresses);
    }
}

/// Lightyear component for a WebTransport endpoint: one bound address that many peers connect to.
///
/// When [`LinkStart`] is triggered, the plugin opens an Aeronet [`WebTransportServer`] at
/// [`bind_addr`](Self::bind_addr) with [`certificate`](Self::certificate) as its TLS identity.
///
/// This wrapper accepts [`SessionRequest`] events automatically. Accepted peers are represented
/// as Lightyear link entities related through [`LinkOf`].
/// Add [`Server`](lightyear_link::server::Server) alongside for the authority role.
#[derive(Debug, Component)]
#[require(Endpoint)]
pub struct WebTransportEndpoint {
    /// Address to bind. Port zero lets the OS choose a port, exposed through [`WebTransportLocalAddr`].
    pub bind_addr: SocketAddr,
    /// TLS identity used by the underlying WebTransport server.
    pub certificate: Identity,
}

impl WebTransportEndpointPlugin {
    fn on_addresses(
        trigger: On<
            Insert,
            (
                LocalAddr,
                PeerAddr,
                AeronetLinkOf,
                WebTransportServer,
                WebTransportServerClient,
            ),
        >,
        query: Query<
            (&AeronetLinkOf, Option<&LocalAddr>, Option<&PeerAddr>),
            Or<(With<WebTransportServer>, With<WebTransportServerClient>)>,
        >,
        mut commands: Commands,
    ) {
        if let Ok((owner, local, peer)) = query.get(trigger.entity) {
            let mut link = commands.entity(owner.0);
            if let Some(local) = local {
                link.insert(WebTransportLocalAddr(local.0));
            }
            if let Some(peer) = peer {
                link.insert(WebTransportPeerAddr(peer.0));
            }
        }
    }

    fn on_linked(
        trigger: On<Add, (Linked, WebTransportLocalAddr, WebTransportPeerAddr)>,
        links: Query<(Entity, &LinkOf, &WebTransportPeerAddr), (With<Linked>, Without<P2P>)>,
        endpoints: Query<&WebTransportLocalAddr, With<WebTransportEndpoint>>,
        mut commands: Commands,
    ) {
        // A bound-address callback can arrive after an accepted session becomes linked.
        let mut admit = |(entity, link_of, peer_addr): (Entity, &LinkOf, &WebTransportPeerAddr)| {
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
        query: Query<(Entity, &WebTransportEndpoint), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, io)) = query.get(trigger.entity) {
            let server_addr = io.bind_addr;
            let certificate = io.certificate.clone_identity();
            commands.queue(move |world: &mut World| {
                let config = ServerConfig::builder()
                    .with_bind_address(server_addr)
                    .with_identity(certificate)
                    .keep_alive_interval(Some(Duration::from_secs(1)))
                    .max_idle_timeout(Some(Duration::from_secs(5)))
                    .expect("should be a valid idle timeout")
                    .build();
                info!("Server WebTransport starting at {}", server_addr);
                let child = world.spawn((AeronetLinkOf(entity), Name::from("WebTransportServer")));
                WebTransportServer::open(config).apply(child);
            });
        }
        Ok(())
    }

    fn on_session_request(mut request: On<SessionRequest>) {
        request.respond(SessionResponse::Accepted);
    }

    fn on_connection(
        trigger: On<Add, (Session, WebTransportServerClient)>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<
            &ChildOf,
            (
                With<WebTransportServerClient>,
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
            commands.entity(trigger.entity).insert((
                AeronetLinkOf(link_entity),
                Name::from("WebTransportClientOf"),
            ));
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

    #[test]
    fn accepted_session_waits_for_bound_and_peer_addresses() {
        use lightyear_connection::client::Connected;

        let mut app = App::new();
        app.add_plugins(WebTransportEndpointPlugin);
        let local = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 4100);
        let peer = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 4101);
        let endpoint = app
            .world_mut()
            .spawn(WebTransportEndpoint {
                bind_addr: local,
                certificate: Identity::self_signed(["localhost"]).unwrap(),
            })
            .id();
        let link = app
            .world_mut()
            .spawn((LinkOf { endpoint }, Link::default(), Linked))
            .id();
        assert!(app.world().get::<Connected>(link).is_none());
        app.world_mut()
            .entity_mut(link)
            .insert(WebTransportPeerAddr(peer));
        assert!(app.world().get::<Connected>(link).is_none());
        app.world_mut()
            .entity_mut(endpoint)
            .insert(WebTransportLocalAddr(local));
        assert!(app.world().get::<Connected>(link).is_some());
        assert_eq!(
            app.world().get::<RemoteId>(link).unwrap().0,
            PeerId::Raw(peer)
        );
    }

    #[cfg(feature = "client")]
    #[test]
    fn local_sessions_respect_admission_ownership_and_disconnect() {
        use crate::client::{WebTransportClientIo, WebTransportClientPlugin};
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
            app.add_plugins(WebTransportEndpointPlugin);
            app.add_observer(|request: On<SessionRequest>| {
                assert!(request.authority.starts_with("localhost:"));
                assert_eq!(request.path, "/session?transport=wt");
            });
            let mut client_app = App::new();
            client_app.add_plugins(WebTransportClientPlugin);
            let address = SocketAddr::new(core::net::Ipv6Addr::UNSPECIFIED.into(), 0);
            let certificate = Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap();
            let digest: String = certificate.certificate_chain().as_slice()[0]
                .hash()
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let server = app
                .world_mut()
                .spawn((
                    Server,
                    WebTransportEndpoint {
                        bind_addr: address,
                        certificate,
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
            let address = app.world().get::<WebTransportLocalAddr>(server).unwrap().0;
            let client = client_app
                .world_mut()
                .spawn(WebTransportClientIo::from_url(
                    format!("https://localhost:{}/session?transport=wt", address.port()),
                    digest,
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
                    .get::<WebTransportPeerAddr>(client)
                    .unwrap()
                    .0
                    .port(),
                address.port()
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
                    PeerId::Raw(app.world().get::<WebTransportPeerAddr>(accepted).unwrap().0),
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

    fn spawn_server(app: &mut App, addr: SocketAddr) -> Entity {
        let entity = app
            .world_mut()
            .spawn(WebTransportEndpoint {
                bind_addr: addr,
                certificate: Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap(),
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
        app.add_plugins(WebTransportEndpointPlugin);

        let server = spawn_server(&mut app, SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0));
        run_app_until(&mut app, |world| world.get::<Linked>(server).is_some());
        let local_addr = app.world().get::<WebTransportLocalAddr>(server).unwrap().0;
        assert_ne!(local_addr.port(), 0);

        app.world_mut().trigger(Unlink {
            entity: server,
            reason: UnlinkReason::UserRequested(Some("test shutdown".to_string())),
        });
        run_app_until(&mut app, |world| {
            world.get::<Unlinked>(server).is_some() && world.get::<AeronetLink>(server).is_none()
        });
        thread::sleep(Duration::from_millis(100));

        let restarted_server = app
            .world_mut()
            .spawn(WebTransportEndpoint {
                bind_addr: local_addr,
                certificate: Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap(),
            })
            .id();
        app.world_mut().trigger(LinkStart {
            entity: restarted_server,
        });
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
