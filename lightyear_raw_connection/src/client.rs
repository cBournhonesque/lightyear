//! Client-side raw connection bridge.
//!
//! A [`crate::client::RawClient`] treats the underlying [`Link`](lightyear_link::Link) lifecycle as
//! the connection lifecycle. When the link becomes [`Linked`](lightyear_link::Linked), the client
//! becomes [`Connected`](lightyear_connection::client::Connected); when the client disconnects, the
//! plugin triggers [`Unlink`](lightyear_link::Unlink).

use aeronet_io::connection::LocalAddr;
use alloc::string::ToString;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use lightyear_connection::client::ConnectionPlugin;
use lightyear_connection::prelude::client::*;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::{Link, LinkSystems, Linked, Unlink};
use lightyear_transport::plugin::TransportSystems;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Plugin that maps raw client link lifecycle to connection lifecycle.
///
/// The plugin installs the base [`ConnectionPlugin`] if needed and orders link, connection, and
/// transport systems so received bytes flow `Link -> Transport` and outgoing bytes flow
/// `Transport -> Link`.
pub struct RawConnectionPlugin;

/// Marker component for a client whose IO link is also its connection.
///
/// In this mode, [`Linked`] and [`Connected`] are equivalent, and
/// [`Unlinked`](lightyear_link::Unlinked) and
/// [`Disconnected`] are equivalent.
///
/// When the link is established, the plugin inserts [`LocalId`] from the link's
/// [`LocalAddr`] and uses [`PeerId::Server`] as the remote peer.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(Link, lightyear_connection::client::Client)]
#[require(Disconnected)]
pub struct RawClient;

impl RawConnectionPlugin {
    /// For RawClients, Linked implies Connected
    fn on_linked(
        trigger: On<Add, Linked>,
        query: Query<&LocalAddr, With<RawClient>>,
        mut commands: Commands,
    ) {
        if let Ok(local_addr) = query.get(trigger.entity) {
            trace!("RawClient Linked! Adding Connected");
            commands.entity(trigger.entity).insert((
                Connected,
                LocalId(PeerId::Raw(local_addr.0)),
                RemoteId(PeerId::Server),
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
                reason: "Client requested".to_string(),
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
