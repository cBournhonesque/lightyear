//! Server-side Aeronet lifecycle bridge.
//!
//! [`ServerAeronetPlugin`](crate::server::ServerAeronetPlugin) mirrors Aeronet server endpoint
//! state onto Lightyear server entities. Concrete Aeronet-backed server transports, such as
//! WebSocket and WebTransport, spawn an Aeronet server entity with
//! [`AeronetLinkOf`](crate::AeronetLinkOf) pointing at the Lightyear
//! [`Server`](lightyear_link::server::Server). This module observes Aeronet open/close events and
//! keeps the Lightyear lifecycle markers in sync.

use alloc::format;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;

use crate::AeronetLinkOf;
use aeronet_io::server::{CloseReason, Closed, Server, ServerEndpoint};
use lightyear_link::server::ServerLinkPlugin;
use lightyear_link::{Linked, Linking, UnlinkReason, Unlinked};
use tracing::trace;

/// Plugin that mirrors Aeronet server endpoint state into Lightyear server link state.
///
/// The plugin ensures [`ServerLinkPlugin`] is installed, then observes Aeronet
/// [`ServerEndpoint`], [`Server`], and [`Closed`] events to insert [`Linking`], [`Linked`], and
/// [`Unlinked`] on the Lightyear server entity.
pub struct ServerAeronetPlugin;

impl ServerAeronetPlugin {
    fn on_opening(
        trigger: On<Add, ServerEndpoint>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(child_of.0)
        {
            trace!(
                "AeronetServer opening for {:?}. Adding Linking on Server",
                child_of.0
            );
            c.insert(Linking);
        }
    }

    fn on_opened(trigger: On<Add, Server>, query: Query<&AeronetLinkOf>, mut commands: Commands) {
        if let Ok(child_of) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(child_of.0)
        {
            trace!(
                "AeronetServer opened for {:?}. Adding Linked on Server",
                child_of.0
            );
            c.insert(Linked);
        }
    }

    fn on_closed(trigger: On<Closed>, query: Query<&AeronetLinkOf>, mut commands: Commands) {
        if let Ok(child_of) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(child_of.0)
        {
            trace!(
                "AeronetServer closed for {:?}. Adding unlinked on Server",
                child_of.0
            );
            let reason = match &trigger.reason {
                CloseReason::ByUser(reason) => UnlinkReason::ClientRequested,
                CloseReason::ByError(err) => UnlinkReason::TransportError(format!("{err:?}")),
            };
            c.insert(Unlinked { reason });
        }
    }
}

impl Plugin for ServerAeronetPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ServerLinkPlugin>() {
            app.add_plugins(ServerLinkPlugin);
        }
        app.add_observer(Self::on_opening);
        app.add_observer(Self::on_opened);
        app.add_observer(Self::on_closed);
    }
}
