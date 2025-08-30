use crate::WebSocketError;
use aeronet_io::Session;
use aeronet_io::connection::{LocalAddr, PeerAddr};
pub use aeronet_websocket::server::{
    Identity, ServerConfig, WebSocketServer, WebSocketServerClient,
};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    error::Result,
    prelude::{
        ChildOf, Commands, Component, Entity, EntityCommand, Name, OnAdd, Query, Trigger, With,
        Without, World,
    },
};
use lightyear_aeronet::server::ServerAeronetPlugin;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::prelude::LinkOf;
use lightyear_link::server::Server;
use lightyear_link::{Link, LinkStart, Linked, Linking};
use tracing::info;

/// Allows using [`WebSocketServer`].
pub struct WebSocketServerPlugin;

impl Plugin for WebSocketServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        if !app.is_plugin_added::<ServerAeronetPlugin>() {
            app.add_plugins(ServerAeronetPlugin);
        }
        app.add_plugins(aeronet_websocket::server::WebSocketServerPlugin);

        app.add_observer(Self::link);
        app.add_observer(Self::on_connection);
    }
}

/// WebSocket server implementation which listens for client connections,
/// and coordinates messaging between multiple clients.
///
/// Use [`WebSocketServer::open`] to start opening a server.
///
/// The [`LocalAddr`] component must be inserted to specify the server_addr.

#[derive(Component)]
#[require(Server)]
pub struct WebSocketServerIo {
    pub config: ServerConfig,
}

impl WebSocketServerPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<
            (Entity, &WebSocketServerIo, Option<&LocalAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, io, local_addr)) = query.get(trigger.target()) {
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
        trigger: Trigger<OnAdd, Session>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<(&ChildOf, &PeerAddr), With<WebSocketServerClient>>,
        mut commands: Commands,
    ) {
        if let Ok((child_of, peer_addr)) = child_query.get(trigger.target())
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
            commands
                .entity(trigger.target())
                .insert((AeronetLinkOf(link_entity), Name::from("WebSocketClientOf")));
        }
    }
}
