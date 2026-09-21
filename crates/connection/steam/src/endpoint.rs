use aeronet_io::Session;
use aeronet_steam::server::{
    ListenTarget, SessionRequest, SessionResponse, SteamNetServer, SteamNetServerClient,
};
use aeronet_steam::{SessionConfig, SteamworksClient};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_aeronet::AeronetLinkOf;
use lightyear_aeronet::endpoint::EndpointAeronetPlugin;
use lightyear_connection::client_of::ClientOf;
use lightyear_link::endpoint::{Endpoint, LinkOf};
use lightyear_link::server::Server;
use lightyear_link::{Link, LinkStart, Linked, Linking, Unlinked};

use crate::SteamSessionFacts;

pub(crate) struct EndpointTransportPlugin;

impl Plugin for EndpointTransportPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EndpointAeronetPlugin>() {
            app.add_plugins(EndpointAeronetPlugin);
        }
        app.add_plugins(aeronet_steam::server::SteamNetServerPlugin);
        app.add_observer(Self::link);
        app.add_observer(Self::on_session_request);
        app.add_observer(Self::on_connection);
        app.add_observer(Self::on_unlinked);
    }
}

/// Steam listen configuration for accepting authenticated Steam sessions.
///
/// The transport accepts native session requests; the selected connection backend decides when
/// the resulting link becomes a Lightyear connection. Add [`Server`] for an authority endpoint.
#[derive(Debug, Component)]
#[require(Endpoint)]
pub struct SteamEndpoint {
    pub target: ListenTarget,
    pub config: SessionConfig,
}

/// A transport-owned link accepted by a [`SteamEndpoint`].
#[derive(Component)]
pub(crate) struct SteamAccepted;

impl EndpointTransportPlugin {
    fn link(
        trigger: On<LinkStart>,
        query: Query<&SteamEndpoint, (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) {
        if let Ok(io) = query.get(trigger.entity) {
            let config = io.config.clone();
            let target = io.target;
            let entity = trigger.entity;
            commands.queue(move |world: &mut World| {
                let session = world.spawn((AeronetLinkOf(entity), Name::from("SteamEndpoint")));
                SteamNetServer::open(config, target).apply(session);
            });
        }
    }

    fn on_session_request(mut request: On<SessionRequest>) {
        request.respond(SessionResponse::Accepted);
    }

    fn on_connection(
        trigger: On<Add, Session>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<(&ChildOf, &SteamNetServerClient)>,
        endpoints: Query<(Has<Server>, Has<lightyear_connection::p2p::P2P>), With<SteamEndpoint>>,
        steam: Option<Res<SteamworksClient>>,
        mut commands: Commands,
    ) {
        if let Ok((child_of, steam_conn)) = child_query.get(trigger.entity)
            && let Ok(endpoint) = query.get(child_of.parent())
            && let Ok((is_server, is_p2p)) = endpoints.get(endpoint.0)
        {
            let steam =
                steam.expect("SteamworksClient must exist for an established Steam session");
            let mut link = commands.spawn((
                LinkOf {
                    endpoint: endpoint.0,
                },
                Link::default(),
                SteamAccepted,
            ));
            if is_p2p {
                link.insert(lightyear_connection::p2p::P2P::default());
            } else if is_server {
                link.insert(ClientOf);
            }
            link.insert(SteamSessionFacts {
                local: steam.user().steam_id(),
                remote: Some(steam_conn.steam_id()),
            });
            let link_entity = link.id();
            commands
                .entity(trigger.entity)
                .insert((AeronetLinkOf(link_entity), Name::from("SteamAccepted")));
        }
    }

    fn on_unlinked(
        trigger: On<Add, Unlinked>,
        query: Query<(), With<SteamAccepted>>,
        mut commands: Commands,
    ) {
        if query.contains(trigger.entity) {
            // Queue after connection observers have translated Unlinked into Disconnected.
            let entity = trigger.entity;
            commands.queue(move |world: &mut World| {
                world.commands().entity(entity).try_despawn();
            });
        }
    }
}
