use aeronet_io::connection::PeerAddr;
use alloc::string::ToString;
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
        trigger: On<Add, Linked>,
        query: Query<(), With<RawServer>>,
        mut commands: Commands,
    ) {
        if query.get(trigger.entity).is_ok() {
            trace!("RawClient Server Linked! Adding Started");
            commands.entity(trigger.entity).insert(Started);
        }
    }

    /// For RawServers, when a LinkOf gets Linked, it also becomes Connected
    fn on_link_of_linked(
        trigger: On<Add, Linked>,
        link_of: Query<(&LinkOf, &PeerAddr)>,
        server: Query<(), With<RawServer>>,
        mut commands: Commands,
    ) {
        if let Ok((link_of, peer_addr)) = link_of.get(trigger.entity)
            && server.get(link_of.server).is_ok()
        {
            trace!("RawClient LinkOf Linked! Adding Connected");
            commands.entity(trigger.entity).insert((
                Connected,
                LocalId(PeerId::Server),
                RemoteId(PeerId::Raw(peer_addr.0)),
                ClientOf,
            ));
        }
    }

    /// For RawServers, Stop implies Unlinked. We also unlink all LinkOfs.
    fn on_stop(
        trigger: On<Stop>,
        mut commands: Commands,
        mut query: Query<&Server, (Without<Stopped>, With<RawServer>)>,
        link_query: Query<(Entity, &RemoteId), (With<ClientOf>, Without<HostClient>)>,
    ) -> Result {
        if let Ok(server) = query.get_mut(trigger.entity) {
            trace!("RawClient Stop! Disconnecting all LinkOfs and triggering Unlink");
            commands.trigger(Unlink {
                entity: trigger.entity,
                reason: "Server stopped".to_string(),
            });
            commands.entity(trigger.entity).insert(Stopping);
            // SAFETY: we know that the list of client entities are unique because it is a Relationship
            let unique_slice =
                unsafe { UniqueEntitySlice::from_slice_unchecked(server.collection()) };
            link_query.iter_many_unique(unique_slice).try_for_each(
                |(entity, remote_peer_id)| {
                    let PeerId::Raw(_) = remote_peer_id.0 else {
                        error!("Client {:?} is not a Netcode client", remote_peer_id);
                        return Err(
                            lightyear_connection::server::ConnectionError::InvalidConnectionType,
                        );
                    };
                    // insert Disconnecting, so that the `Disconnected` component is added on the LinkOf
                    // before the entity gets despawned
                    commands.entity(entity).insert(Disconnecting);
                    Ok(())
                },
            )?;
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
