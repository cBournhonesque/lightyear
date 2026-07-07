//! Server-side WebTransport transport integration.
//!
//! [`WebTransportServerIo`](crate::server::WebTransportServerIo) is inserted on a Lightyear server
//! entity. When [`LinkStart`](lightyear_link::LinkStart) is triggered, the plugin spawns an Aeronet
//! WebTransport server entity and relates it back to the Lightyear server. Each accepted Aeronet
//! session receives its own child Lightyear [`Link`](lightyear_link::Link) through
//! [`LinkOf`](lightyear_link::prelude::LinkOf).

use crate::WebTransportError;
use aeronet_io::Session;
use aeronet_io::connection::{LocalAddr, PeerAddr};
use aeronet_webtransport::server::{
    ServerConfig, SessionRequest, SessionResponse, WebTransportServer, WebTransportServerClient,
};
use aeronet_webtransport::wtransport::Identity;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use core::time::Duration;
use lightyear_aeronet::server::ServerAeronetPlugin;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::prelude::LinkOf;
use lightyear_link::server::Server;
use lightyear_link::{Link, LinkStart, Linked, Linking};
use tracing::info;

/// Plugin that starts WebTransport servers and creates per-client Lightyear links.
///
/// The plugin installs [`AeronetPlugin`], [`ServerAeronetPlugin`], and Aeronet's WebTransport
/// server plugin. It observes [`LinkStart`] for [`WebTransportServerIo`] entities, accepts
/// [`SessionRequest`] by default, and observes new Aeronet sessions to create the corresponding
/// child [`Link`] entities.
pub struct WebTransportServerPlugin;

impl Plugin for WebTransportServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        if !app.is_plugin_added::<ServerAeronetPlugin>() {
            app.add_plugins(ServerAeronetPlugin);
        }
        app.add_plugins(aeronet_webtransport::server::WebTransportServerPlugin);

        app.add_observer(Self::link);
        app.add_observer(Self::on_session_request);
        app.add_observer(Self::on_connection);
    }
}

/// Lightyear component for a WebTransport server endpoint.
///
/// Insert this on a Lightyear server entity. A [`LocalAddr`] must be present when [`LinkStart`] is
/// triggered; the plugin opens an Aeronet [`WebTransportServer`] with
/// [`certificate`](Self::certificate) as its TLS identity.
///
/// This wrapper currently accepts [`SessionRequest`] events automatically. Accepted clients are
/// represented as child Lightyear link entities related to the server through [`LinkOf`].
#[derive(Debug, Component)]
#[require(Server)]
pub struct WebTransportServerIo {
    /// TLS identity used by the underlying WebTransport server.
    pub certificate: Identity,
}

impl WebTransportServerPlugin {
    fn link(
        trigger: On<LinkStart>,
        query: Query<
            (Entity, &WebTransportServerIo, Option<&LocalAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, io, local_addr)) = query.get(trigger.entity) {
            let server_addr = local_addr.ok_or(WebTransportError::LocalAddrMissing)?.0;
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

    // TODO: should also add on_connecting? Or maybe it's handled automatically
    //  because the connecting entity adds SessionEndpoint? (and lightyear_aeronet handles that)
    fn on_connection(
        trigger: On<Add, Session>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<(&ChildOf, &PeerAddr), With<WebTransportServerClient>>,
        mut commands: Commands,
    ) {
        if let Ok((child_of, peer_addr)) = child_query.get(trigger.entity)
            && let Ok(server_link) = query.get(child_of.parent())
        {
            let link_entity = commands
                .spawn((
                    LinkOf {
                        server: server_link.0,
                    },
                    Link::new(None),
                    PeerAddr(peer_addr.0),
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
    use std::{thread, time::Duration};

    use lightyear_aeronet::AeronetLink;
    use lightyear_link::{LinkStart, Unlink, Unlinked};

    fn spawn_server(app: &mut App, addr: SocketAddr) -> Entity {
        let entity = app
            .world_mut()
            .spawn((
                LocalAddr(addr),
                WebTransportServerIo {
                    certificate: Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap(),
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
        app.add_plugins(WebTransportServerPlugin);

        let server = spawn_server(&mut app, SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0));
        run_app_until(&mut app, |world| world.get::<Linked>(server).is_some());
        let local_addr = app.world().get::<LocalAddr>(server).unwrap().0;
        assert_ne!(local_addr.port(), 0);

        app.world_mut().trigger(Unlink {
            entity: server,
            reason: "test shutdown".to_string(),
        });
        run_app_until(&mut app, |world| {
            world.get::<Unlinked>(server).is_some() && world.get::<AeronetLink>(server).is_none()
        });
        thread::sleep(Duration::from_millis(100));

        let restarted_server = app
            .world_mut()
            .spawn((
                LocalAddr(local_addr),
                WebTransportServerIo {
                    certificate: Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap(),
                },
            ))
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
