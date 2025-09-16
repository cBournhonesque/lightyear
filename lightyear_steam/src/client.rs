use aeronet_io::connection::PeerAddr;
use aeronet_steam::client::SteamNetClient;
use aeronet_steam::{
    SessionConfig, SteamworksClient,
    client::{ConnectTarget, SteamNetClientPlugin},
};
use alloc::string::ToString;
use bevy_app::{App, Plugin};
use bevy_ecs::component::HookContext;
use bevy_ecs::prelude::{OnAdd, With};
use bevy_ecs::world::DeferredWorld;
use bevy_ecs::{
    error::Result,
    prelude::{Commands, Component, Entity, EntityCommand, Name, Query, Trigger, Without, World},
};
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_connection::client::{Connected, Disconnect};
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::{Link, LinkStart, Linked, Linking, Unlink};
use tracing::trace;

pub struct SteamClientPlugin;

impl Plugin for SteamClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        app.add_plugins(SteamNetClientPlugin);
        app.add_observer(Self::link);
        app.add_observer(Self::on_linked);
        app.add_observer(Self::disconnect);
    }
}

/// Component to insert on an entity to start a Steam  socket client which
/// can connect to a dedicated server or another peer.
///
/// The [`SteamworksClient`] resource must have been created beforehand.
#[derive(Debug, Component)]
#[component(on_add = SteamClientIo::on_add)]
#[require(Link)]
pub struct SteamClientIo {
    pub target: ConnectTarget,
    pub config: SessionConfig,
}

impl SteamClientIo {
    /// When we add the SteamClientIo component, we will also add a LocalId component
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        let local_steam_id = world.resource::<SteamworksClient>().0.user().steam_id();
        world
            .commands()
            .entity(entity)
            .insert(LocalId(PeerId::Steam(local_steam_id.raw())));
    }
}

impl SteamClientPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<(Entity, &SteamClientIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, client)) = query.get(trigger.target()) {
            let config = client.config.clone();
            let target = client.target;
            trace!(
                "LinkStart triggered for SteamClientIo on entity {entity:?} with target {target:?}"
            );
            commands.queue(move |world: &mut World| -> Result {
                let mut link_entity_mut = world.entity_mut(entity);
                match target {
                    ConnectTarget::Addr(peer_addr) => {
                        link_entity_mut.insert(PeerAddr(peer_addr));

                        // TODO: we need a RemoteId here. Maybe SteamP2PAddr?
                        link_entity_mut.insert(RemoteId(PeerId::Server));
                    }
                    ConnectTarget::Peer { steam_id, .. } => {
                        link_entity_mut.insert(RemoteId(PeerId::Steam(steam_id.raw())));
                    }
                }
                let entity_mut =
                    world.spawn((
                        AeronetLinkOf(entity),
                        Name::from("SteamClient")
                    ));
                let aeronet_entity = entity_mut.id();
                trace!("Starting LinkStart for SteamClientIo on entity {entity:?}. Spawning aeronet entity: {aeronet_entity:?}");

                // TODO: also add LocalAddr or LocalId components
                SteamNetClient::connect(config, target).apply(entity_mut);
                Ok(())
            });
        }
        Ok(())
    }

    /// Steam is both a Link and a Connection, so we add Connected when Linked is added
    fn on_linked(
        trigger: Trigger<OnAdd, Linked>,
        query: Query<(), With<SteamClientIo>>,
        mut commands: Commands,
    ) {
        if query.get(trigger.target()).is_ok() {
            trace!(
                "Steam client entity {:?} is now Linked, inserting Connected component",
                trigger.target()
            );
            commands.entity(trigger.target()).insert(Connected);
        }
    }

    /// Steam is both a Link and a Connection, so we trigger Unlink when Disconnect is triggered
    fn disconnect(
        trigger: Trigger<Disconnect>,
        query: Query<(), With<SteamClientIo>>,
        mut commands: Commands,
    ) {
        if query.get(trigger.target()).is_ok() {
            commands.trigger_targets(
                Unlink {
                    reason: "User requested disconnect".to_string(),
                },
                trigger.target(),
            );
        }
    }
}
