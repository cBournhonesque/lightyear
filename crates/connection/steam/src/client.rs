use aeronet_io::Session;
use aeronet_steam::{
    SessionConfig, SteamworksClient,
    client::{ConnectTarget, SteamNetClient, SteamNetClientPlugin},
};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_aeronet::AeronetLinkOf;
use lightyear_link::{Link, LinkStart, Linked, Linking, Unlinked};

use crate::SteamSessionFacts;

pub(crate) struct ClientTransportPlugin;

impl Plugin for ClientTransportPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SteamNetClientPlugin);
        app.add_observer(Self::link);
        app.add_observer(Self::session);
        app.add_observer(Self::unlinked);
    }
}

/// Native Steam dial configuration for a dedicated server or another peer.
///
/// The [`SteamworksClient`] resource must exist before starting the link. Routing identities are
/// resolved by the connection backend only after Steam establishes the session.
#[derive(Debug, Component)]
#[require(Link)]
pub struct SteamClientIo {
    pub target: ConnectTarget,
    pub config: SessionConfig,
}

impl ClientTransportPlugin {
    fn unlinked(
        trigger: On<Add, Unlinked>,
        query: Query<(), With<SteamClientIo>>,
        mut commands: Commands,
    ) {
        if query.contains(trigger.entity) {
            commands
                .entity(trigger.entity)
                .remove::<SteamSessionFacts>();
        }
    }

    fn link(
        trigger: On<LinkStart>,
        query: Query<&SteamClientIo, (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) {
        if let Ok(client) = query.get(trigger.entity) {
            let config = client.config.clone();
            let target = client.target;
            let entity = trigger.entity;
            commands.queue(move |world: &mut World| {
                let session = world.spawn((AeronetLinkOf(entity), Name::from("SteamClient")));
                SteamNetClient::connect(config, target).apply(session);
            });
        }
    }

    fn session(
        trigger: On<Add, (Session, AeronetLinkOf)>,
        sessions: Query<&AeronetLinkOf, (With<Session>, With<SteamNetClient>)>,
        clients: Query<&SteamClientIo>,
        steam: Option<Res<SteamworksClient>>,
        mut commands: Commands,
    ) {
        if let Ok(link) = sessions.get(trigger.entity)
            && let Ok(client) = clients.get(link.0)
        {
            let steam =
                steam.expect("SteamworksClient must exist for an established Steam session");
            let remote = match client.target {
                ConnectTarget::Peer { steam_id, .. } => Some(steam_id),
                // Aeronet does not expose outgoing session identity. A socket target is not a
                // Steam identity; the Steam connection backend uses its C/S role instead.
                ConnectTarget::Addr(_) => None,
            };
            commands.entity(link.0).insert(SteamSessionFacts {
                local: steam.user().steam_id(),
                remote,
            });
        }
    }
}
