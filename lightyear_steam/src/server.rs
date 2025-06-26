use aeronet_io::Session;
use aeronet_steam::SessionConfig;
use aeronet_steam::server::{
    ListenTarget, SessionRequest, SessionResponse, SteamNetServer, SteamNetServerClient,
};
use aeronet_steam::steamworks::ServerManager;
use bevy_app::{App, Plugin};
use bevy_ecs::{
    error::Result,
    prelude::{
        ChildOf, Commands, Component, Entity, EntityCommand, Name, OnAdd, Query, Trigger, Without,
        World,
    },
};
use lightyear_aeronet::server::ServerAeronetPlugin;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_link::prelude::LinkOf;
use lightyear_link::server::Server;
use lightyear_link::{Link, LinkStart, Linked, Linking};
use tracing::info;

/// Enables starting a Steam server
pub struct SteamServerPlugin;

impl Plugin for SteamServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        if !app.is_plugin_added::<ServerAeronetPlugin>() {
            app.add_plugins(ServerAeronetPlugin);
        }
        app.add_plugins(aeronet_steam::server::SteamNetServerPlugin::<ServerManager>::default());

        app.add_observer(Self::link);
        app.add_observer(Self::on_session_request);
        app.add_observer(Self::on_connection);
    }
}

/// WebTransport server implementation which listens for client connections,
/// and coordinates messaging between multiple clients.
///
/// When a client attempts to connect, the server will trigger a
/// [`SessionRequest`]. Your app **must** observe this, and use
/// [`SessionRequest::respond`] to set how the server should respond to this
/// connection attempt.
#[derive(Debug, Component)]
#[require(Server)]
pub struct SteamServerIo {
    pub target: ListenTarget,
    pub config: SessionConfig,
}

impl SteamServerPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<(Entity, &SteamServerIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, io)) = query.get(trigger.target()) {
            let config = io.config.clone();
            let target = io.target;
            commands.queue(move |world: &mut World| {
                info!("Server Steam starting at {:?}", target);
                let child = world.spawn((AeronetLinkOf(entity), Name::from("SteamServer")));
                SteamNetServer::<ServerManager>::open(config, target).apply(child);
            });
        }
        Ok(())
    }

    fn on_session_request(mut request: Trigger<SessionRequest>) {
        request.respond(SessionResponse::Accepted);
    }

    fn on_connection(
        trigger: Trigger<OnAdd, Session>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<(&ChildOf, &SteamNetServerClient<ServerManager>)>,
        mut commands: Commands,
    ) {
        if let Ok((child_of, steam_conn)) = child_query.get(trigger.target()) {
            if let Ok(server_link) = query.get(child_of.parent()) {
                let link_entity = commands
                    .spawn((
                        LinkOf {
                            server: server_link.0,
                        },
                        Link::new(None),
                        RemoteId(PeerId::Steam(steam_conn.steam_id().raw())),
                    ))
                    .id();
                commands
                    .entity(trigger.target())
                    .insert((AeronetLinkOf(link_entity), Name::from("SteamClientOf")));
            }
        }
    }
}
