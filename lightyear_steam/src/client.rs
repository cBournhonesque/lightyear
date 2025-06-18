use aeronet_io::connection::PeerAddr;
use aeronet_steam::client::SteamNetClient;
use aeronet_steam::steamworks::ClientManager;
use aeronet_steam::{
    SessionConfig,
    client::{ConnectTarget, SteamNetClientPlugin},
};
use bevy::prelude::*;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_link::{Link, LinkStart, Linked, Linking};

pub struct SteamClientPlugin;

impl Plugin for SteamClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        app.add_plugins(SteamNetClientPlugin::<ClientManager>::default());
        app.add_observer(Self::link);
    }
}

/// Component to insert on an entity to start a Steam  socket client which
/// can connect to a dedicated server or another peer.
///
/// The [`SteamworksClient`] resource must have been created beforehand.
#[derive(Debug, Component)]
#[require(Link)]
pub struct SteamClientIo {
    target: ConnectTarget,
    config: SessionConfig,
}

impl SteamClientPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<(Entity, &SteamClientIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, client)) = query.get(trigger.target()) {
            let config = client.config.clone();
            let target = client.target.clone();
            commands.queue(move |world: &mut World| -> Result {
                let mut entity_mut =
                    world.spawn((AeronetLinkOf(entity), Name::from("SteamClient")));
                match target {
                    ConnectTarget::Addr(peer_addr) => {
                        entity_mut.insert(PeerAddr(peer_addr));
                    }
                    ConnectTarget::Peer { steam_id, .. } => {
                        entity_mut.insert(RemoteId(PeerId::Steam(steam_id.raw())));
                    }
                }
                // TODO: also add LocalAddr or LocalId components
                SteamNetClient::<ClientManager>::connect(config, target).apply(entity_mut);
                Ok(())
            });
        }
        Ok(())
    }
}
