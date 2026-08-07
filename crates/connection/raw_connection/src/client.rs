use aeronet_io::connection::LocalAddr;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use lightyear_connection::client::ConnectionPlugin;
use lightyear_connection::prelude::client::*;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::{Link, LinkSystems, Linked, Unlink, UnlinkReason};
use lightyear_transport::plugin::TransportSystems;
#[allow(unused_imports)]
use tracing::{info, trace};

pub struct RawConnectionPlugin;

/// Marker type to represent a client where the IO layer (UDP/Websocket/WebTransport/etc.) which also acts as a Connection layer
///
/// In this case, Linked/Connected are equivalent; same for Unlinked/Disconnected.
///
/// The PeerId associated with the connection is the entity itself.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(Link, lightyear_connection::client::Client)]
#[require(Disconnected)]
pub struct RawClient;

impl RawConnectionPlugin {
    /// For RawClients, Linked implies Connected
    fn on_linked(
        trigger: On<Add, Linked>,
        query: Query<(&LocalAddr, Option<&LocalId>, Option<&RemoteId>), With<RawClient>>,
        mut commands: Commands,
    ) {
        if let Ok((local_addr, local_id, remote_id)) = query.get(trigger.entity) {
            trace!("RawClient Linked! Adding Connected");
            commands.entity(trigger.entity).insert((
                Connected,
                local_id
                    .copied()
                    .unwrap_or(LocalId(PeerId::Raw(local_addr.0))),
                remote_id.copied().unwrap_or(RemoteId(PeerId::Server)),
            ));
        }
    }

    /// For RawClients, Disconnect implies Unlinked
    fn on_disconnect(
        trigger: On<Disconnect>,
        mut commands: Commands,
        mut query: Query<(), (Without<Disconnected>, With<RawClient>)>,
    ) {
        if query.get_mut(trigger.entity).is_ok() {
            trace!("RawClient Disconnect! Triggering Unlink");
            commands.trigger(Unlink {
                entity: trigger.entity,
                reason: UnlinkReason::UserRequested(None),
            });
        }
    }
}

impl Plugin for RawConnectionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ConnectionPlugin>() {
            app.add_plugins(ConnectionPlugin);
        }
        app.configure_sets(
            PreUpdate,
            (LinkSystems::Receive, TransportSystems::Receive).chain(),
        );
        app.configure_sets(
            PostUpdate,
            (TransportSystems::Send, LinkSystems::Send).chain(),
        );
        app.add_observer(Self::on_linked);
        app.add_observer(Self::on_disconnect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_client_preserves_preconfigured_peer_ids() {
        let mut app = App::new();
        app.add_plugins(RawConnectionPlugin);
        let local_id = LocalId(PeerId::Entity(1));
        let remote_id = RemoteId(PeerId::Entity(2));
        let entity = app
            .world_mut()
            .spawn((
                RawClient,
                LocalAddr("127.0.0.1:10001".parse().unwrap()),
                local_id,
                remote_id,
            ))
            .id();

        app.world_mut().entity_mut(entity).insert(Linked);
        app.world_mut().flush();

        let entity = app.world().entity(entity);
        assert!(entity.contains::<Connected>());
        assert_eq!(entity.get::<LocalId>(), Some(&local_id));
        assert_eq!(entity.get::<RemoteId>(), Some(&remote_id));
    }
}
