use aeronet_io::connection::PeerAddr;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::entity::UniqueEntitySlice;
use bevy_ecs::prelude::*;
use core::fmt::Debug;
use lightyear_connection::prelude::client::*;
use lightyear_connection::prelude::server::*;
use lightyear_link::prelude::*;

use lightyear_connection::client::Disconnecting;
use lightyear_connection::host::HostClient;
use lightyear_connection::server::Stopping;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_transport::plugin::TransportSystems;
#[allow(unused_imports)]
use tracing::{error, trace};

pub struct RawConnectionPlugin;

/// Marker type to represent a server where the IO layer (UDP/Websocket/WebTransport/etc.) which also acts as a Connection layer
///
/// In this case, Linked/Started are equivalent; same for Unlinked/Stopped.
///
/// The PeerId associated with the connection is the entity itself.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(Server)]
pub struct RawServer;

impl RawConnectionPlugin {
    /// For RawServers, Linked implies Started
    fn on_server_linked(
        trigger: On<Add<Linked>>,
        query: Query<(), With<RawServer>>,
        mut commands: Commands,
    ) {
        if query.get(trigger.entity).is_ok() {
            trace!("RawClient Server Linked! Adding Started");
            commands.entity(trigger.entity).insert(Started);
        }
    }

    /// For RawServers, when a LinkOf gets Linked, it also becomes Connected
    ///
    /// A `RemoteId` the IO layer already put on the link is kept, as for a `RawClient`: a
    /// connection-oriented IO layer can carry several links from one `PeerAddr`, so it names
    /// each peer itself.
    fn on_link_of_linked(
        trigger: On<Add<Linked>>,
        link_of: Query<(&LinkOf, &PeerAddr, Option<&RemoteId>)>,
        server: Query<(), With<RawServer>>,
        mut commands: Commands,
    ) {
        if let Ok((link_of, peer_addr, remote_id)) = link_of.get(trigger.entity)
            && server.get(link_of.endpoint).is_ok()
        {
            trace!("RawClient LinkOf Linked! Adding Connected");
            commands.entity(trigger.entity).insert((
                Connected,
                LocalId(PeerId::Server),
                remote_id
                    .copied()
                    .unwrap_or(RemoteId(PeerId::Raw(peer_addr.0))),
                ClientOf,
            ));
        }
    }

    /// For RawServers, Stop implies Unlinked. We also unlink all LinkOfs.
    fn on_stop(
        trigger: On<Stop>,
        mut commands: Commands,
        mut query: Query<&Endpoint, (Without<Stopped>, With<RawServer>)>,
        link_query: Query<(Entity, &RemoteId), (With<ClientOf>, Without<HostClient>)>,
    ) -> Result {
        if let Ok(server) = query.get_mut(trigger.entity) {
            trace!("RawClient Stop! Disconnecting all LinkOfs and triggering Unlink");
            commands.trigger(Unlink {
                entity: trigger.entity,
                reason: UnlinkReason::ServerStopped,
            });
            commands.entity(trigger.entity).insert(Stopping);
            // SAFETY: we know that the list of client entities are unique because it is a Relationship
            let unique_slice =
                unsafe { UniqueEntitySlice::from_slice_unchecked(server.collection()) };
            link_query
                .iter_many_unique(unique_slice)
                .matched()
                .try_for_each(|(entity, remote_peer_id)| {
                    let PeerId::Raw(_) = remote_peer_id.0 else {
                        error!("Client {:?} is not a Netcode client", remote_peer_id);
                        return Err(
                            lightyear_connection::server::ConnectionError::InvalidConnectionType,
                        );
                    };
                    // The transport may already have queued a despawn for this link.
                    // Otherwise, insert Disconnecting so Disconnected is added before despawn.
                    commands.entity(entity).try_insert(Disconnecting);
                    Ok(())
                })?;
        }
        Ok(())
    }
}

impl Plugin for RawConnectionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lightyear_connection::client::ConnectionPlugin>() {
            app.add_plugins(lightyear_connection::client::ConnectionPlugin);
        }
        if !app.is_plugin_added::<lightyear_connection::server::ConnectionPlugin>() {
            app.add_plugins(lightyear_connection::server::ConnectionPlugin);
        }
        app.configure_sets(
            PreUpdate,
            (LinkSystems::Receive, TransportSystems::Receive).chain(),
        );
        app.configure_sets(
            PostUpdate,
            (TransportSystems::Send, LinkSystems::Send).chain(),
        );
        app.add_observer(Self::on_server_linked);
        app.add_observer(Self::on_link_of_linked);
        app.add_observer(Self::on_stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Despawns clients while raw-server stop commands are still deferred.
    fn despawn_client_on_stop(
        _trigger: On<Stop>,
        clients: Query<Entity, With<ClientOf>>,
        mut commands: Commands,
    ) {
        for client in &clients {
            commands.entity(client).despawn();
        }
    }

    #[test]
    fn stopping_raw_server_tolerates_already_despawned_client() {
        let mut app = App::new();
        app.add_observer(despawn_client_on_stop);
        app.add_plugins(RawConnectionPlugin);

        let server = app.world_mut().spawn(RawServer).id();
        let client = app
            .world_mut()
            .spawn((
                LinkOf { endpoint: server },
                ClientOf,
                RemoteId(PeerId::Raw("127.0.0.1:5000".parse().unwrap())),
            ))
            .id();

        app.world_mut().trigger(Stop { entity: server });
        app.world_mut().flush();

        assert!(app.world().get_entity(client).is_err());
    }

    #[test]
    fn raw_server_link_preserves_preconfigured_remote_id() {
        let mut app = App::new();
        app.add_plugins(RawConnectionPlugin);
        let server = app.world_mut().spawn(RawServer).id();
        let addr = "127.0.0.1:5000".parse().unwrap();
        let named = RemoteId(PeerId::Entity(7));
        let named_link = app
            .world_mut()
            .spawn((LinkOf { endpoint: server }, PeerAddr(addr), named))
            .id();
        let addressed_link = app
            .world_mut()
            .spawn((LinkOf { endpoint: server }, PeerAddr(addr)))
            .id();

        app.world_mut().entity_mut(named_link).insert(Linked);
        app.world_mut().entity_mut(addressed_link).insert(Linked);
        app.world_mut().flush();

        let named_link = app.world().entity(named_link);
        assert!(named_link.contains::<Connected>());
        assert_eq!(named_link.get::<RemoteId>(), Some(&named));
        let addressed_link = app.world().entity(addressed_link);
        assert!(addressed_link.contains::<Connected>());
        assert_eq!(
            addressed_link.get::<RemoteId>(),
            Some(&RemoteId(PeerId::Raw(addr)))
        );
    }
}
