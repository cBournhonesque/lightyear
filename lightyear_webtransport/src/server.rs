use aeronet_io::Session;
use aeronet_io::connection::PeerAddr;
use aeronet_webtransport::server::{
    ServerConfig, SessionRequest, SessionResponse, WebTransportServer, WebTransportServerClient,
};
use aeronet_webtransport::wtransport::Identity;
use bevy::prelude::*;
use core::net::SocketAddr;
use core::time::Duration;
use lightyear_aeronet::server::ServerAeronetPlugin;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_link::prelude::LinkOf;
use lightyear_link::server::Server;
use lightyear_link::{Link, LinkStart, Linked, Linking, Unlinked};

/// Allows using [`WebTransportServer`].
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

/// WebTransport server implementation which listens for client connections,
/// and coordinates messaging between multiple clients.
///
/// Use [`WebTransportServer::open`] to start opening a server.
///
/// When a client attempts to connect, the server will trigger a
/// [`SessionRequest`]. Your app **must** observe this, and use
/// [`SessionRequest::respond`] to set how the server should respond to this
/// connection attempt.
#[derive(Debug, Component)]
#[require(Server)]
pub struct WebTransportServerIo {
    pub server_addr: SocketAddr,
    pub certificate: Identity,
}

impl WebTransportServerPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<(Entity, &WebTransportServerIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) {
        if let Ok((entity, io)) = query.get(trigger.target()) {
            let addr = io.server_addr;
            let certificate = io.certificate.clone_identity();
            commands.queue(move |world: &mut World| {
                let config = ServerConfig::builder()
                    .with_bind_address(addr)
                    .with_identity(certificate)
                    .keep_alive_interval(Some(Duration::from_secs(1)))
                    .max_idle_timeout(Some(Duration::from_secs(5)))
                    .expect("should be a valid idle timeout")
                    .build();
                info!("Server WebTransport starting at {}", addr);
                let child = world.spawn((AeronetLinkOf(entity), Name::from("WebTransportServer")));
                WebTransportServer::open(config).apply(child);
            });
        }
    }

    fn on_session_request(mut request: Trigger<SessionRequest>) {
        request.respond(SessionResponse::Accepted);
    }

    fn on_connection(
        trigger: Trigger<OnAdd, Session>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<(&ChildOf, &PeerAddr), With<WebTransportServerClient>>,
        mut commands: Commands,
    ) {
        if let Ok((child_of, peer_addr)) = child_query.get(trigger.target()) {
            if let Ok(server_link) = query.get(child_of.parent()) {
                let link = Link::new(peer_addr.0, None);
                let link_entity = commands
                    .spawn((
                        LinkOf {
                            server: server_link.0,
                        },
                        link,
                    ))
                    .id();
                commands.entity(trigger.target()).insert((
                    AeronetLinkOf(link_entity),
                    Name::from("WebTransportClientOf"),
                ));
            }
        }
    }
}
