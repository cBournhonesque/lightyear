use aeronet_io::server::Close;
use aeronet_steam::server::{
    ListenTarget, SessionRequest, SessionResponse, SteamNetServer, SteamNetServerClient,
};
#[allow(unused_imports)]
use aeronet_steam::steamworks::{ClientManager, ServerManager};
use alloc::format;
use alloc::string::ToString;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::With;
use bevy_ecs::relationship::RelationshipTarget;
use bevy_ecs::{
    error::Result,
    prelude::{
        ChildOf, Commands, Component, Entity, EntityCommand, Name, OnAdd, Query, Trigger, Without,
        World,
    },
};
use lightyear_aeronet::server::ServerAeronetPlugin;
use lightyear_aeronet::{AeronetLink, AeronetLinkOf, AeronetPlugin};
use lightyear_connection::client::{Connected, Disconnected};
use lightyear_connection::client_of::{ClientOf, SkipNetcode};
use lightyear_connection::server::{Start, Started, Stop};
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_link::prelude::LinkOf;
use lightyear_link::server::Server;
use lightyear_link::{Link, LinkStart, Linked, Linking};
use tracing::{info, trace};

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
        // TODO: should i be using ClientManager or ServerManager here? Maybe ServerManager for dedicated server, ClientManager for peer-to-peer?
        app.add_plugins(aeronet_steam::server::SteamNetServerPlugin::<ClientManager>::default());

        app.add_observer(Self::link);
        app.add_observer(Self::on_linked);
        app.add_observer(Self::start);
        app.add_observer(Self::on_session_request);
        // app.add_observer(Self::on_connecting);
        app.add_observer(Self::on_connection);
        app.add_observer(Self::on_disconnected);
        app.add_observer(Self::stop);
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

/// Marker component to identify this link as connected to a Steam Server.
#[derive(Component)]
// Steam links don't need to go through Netcode, so we skip it
#[require(SkipNetcode)]
pub struct SteamClientOf;

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
                SteamNetServer::<ClientManager>::open(config, target).apply(child);
            });
        }
        Ok(())
    }

    /// Steam is both a Link and a Connection, so we add Started when Linked is added
    fn on_linked(
        trigger: Trigger<OnAdd, Linked>,
        query: Query<(), With<SteamServerIo>>,
        mut commands: Commands,
    ) {
        if query.get(trigger.target()).is_ok() {
            commands.entity(trigger.target()).insert(Started);
        }
    }

    /// Steam is both a Link and a Connection, so on a Start trigger we will just try to LinkStart.
    fn start(
        trigger: Trigger<Start>,
        query: Query<(), (Without<Linking>, Without<Linked>, With<SteamServerIo>)>,
        mut commands: Commands,
    ) {
        if query.get(trigger.target()).is_ok() {
            trace!("SteamServer Start triggered, triggering LinkStart");
            commands.trigger_targets(LinkStart, trigger.target());
        }
    }

    fn on_session_request(mut request: Trigger<SessionRequest>) {
        trace!("Accepted steam link-of request: {:?}", request.target());
        request.respond(SessionResponse::Accepted);
    }

    // fn on_connecting(
    //     // SteamNetServerClient is added at the same time as SessionEndpoint
    //     trigger: Trigger<OnAdd, SteamNetServerClient<ClientManager>>,
    //     query: Query<&AeronetLinkOf>,
    //     child_query: Query<(&ChildOf, &SteamNetServerClient<ClientManager>)>,
    //     mut commands: Commands,
    // ) {
    //     if let Ok((child_of, steam_conn)) = child_query.get(trigger.target()) {
    //         if let Ok(server_link) = query.get(child_of.parent()) {
    //             let link_entity = commands
    //                 .spawn((
    //                     LinkOf {
    //                         server: server_link.0,
    //                     },
    //                     Link::new(None),
    //                     ClientOf,
    //                     Linked,
    //                     Connecting,
    //                     RemoteId(PeerId::Steam(steam_conn.steam_id().raw())),
    //                     SteamClientOf,
    //                 ))
    //                 .id();
    //             trace!(
    //                 "New Steam LinkOf connecting. AeronetEntity: {:?}, LinkOf: {:?}. Steam id: {:?}",
    //                 trigger.target(),
    //                 link_entity,
    //                 steam_conn.steam_id()
    //             );
    //             commands
    //                 .entity(trigger.target())
    //                 .insert((AeronetLinkOf(link_entity), Name::from("SteamClientOf")));
    //         }
    //     }
    // }
    //
    // fn on_connection(
    //     trigger: Trigger<OnAdd, Session>,
    //     link_of_query: Query<&AeronetLinkOf, With<SteamNetServerClient<ClientManager>>>,
    //     mut commands: Commands,
    // ) {
    //     info!("steam connection");
    //     if let Ok(link_of) = link_of_query.get(trigger.target()) {
    //         if let Ok(mut link) = commands.get_entity(link_of.0) {
    //             trace!(
    //                 "Steam link-of connection established. AeronetEntity: {:?}, LinkOf: {:?}", trigger.target(), link_of.0
    //             );
    //             link.insert(Connected);
    //         }
    //     }
    // }

    fn on_connection(
        trigger: Trigger<OnAdd, Session>,
        query: Query<&AeronetLinkOf>,
        child_query: Query<(&ChildOf, &SteamNetServerClient<ClientManager>)>,
        mut commands: Commands,
    ) {
        if let Ok((child_of, steam_conn)) = child_query.get(trigger.target()) {
            if let Ok(server_link) = query.get(child_of.parent()) {
                trace!(
                    "New Steam connection established with client that has SteamId: {:?}",
                    steam_conn.steam_id()
                );
                let link_entity = commands
                    .spawn((
                        LinkOf {
                            server: server_link.0,
                        },
                        Link::new(None),
                        ClientOf,
                        Connected,
                        SteamClientOf,
                        RemoteId(PeerId::Steam(steam_conn.steam_id().raw())),
                    ))
                    .id();
                commands
                    .entity(trigger.target())
                    .insert((AeronetLinkOf(link_entity), Name::from("SteamClientOf")));
            }
        }
    }

    /// Steam is both a Link and a Connection, so on a aeronet Disconnected we trigger Unlinked, Disconnected and we despawn the entity
    fn on_disconnected(
        trigger: Trigger<aeronet_io::connection::Disconnected>,
        query: Query<&AeronetLinkOf, With<SteamNetServerClient<ClientManager>>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link_of) = query.get(trigger.target()) {
            if let Ok(mut link_of_entity) = commands.get_entity(aeronet_link_of.0) {
                trace!(
                    "Aeronet SteamClientOf entity {:?} disconnected: {:?}. Disconnecting and despawning LinkOf entity {:?}.",
                    trigger.target(),
                    trigger,
                    link_of_entity.id()
                );
                link_of_entity
                    // to avoid warnings if we delete the Aeronet entity before the deletion trigger can run by aeronet
                    // Can remove if https://github.com/aecsocket/aeronet/pull/49 is merged
                    .remove::<AeronetLink>()
                    .insert(Disconnected {
                        reason: Some(format!("Aeronet link disconnected: {trigger:?}")),
                    })
                    .try_despawn();
            }
        }
    }

    // TODO: how do we make sure that the host-client is not despawned?
    /// Steam is both a Link and a Connection, so on an Stop we also trigger Close.
    /// This will despawn the underlying steam entity.
    fn stop(
        trigger: Trigger<Stop>,
        query: Query<&AeronetLink, With<SteamServerIo>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.target()) {
            trace!("SteamServer Stop triggered, closing.");
            commands.trigger_targets(
                Close {
                    reason: "User requested".to_string(),
                },
                *aeronet_link.collection(),
            );
        }
    }
}
