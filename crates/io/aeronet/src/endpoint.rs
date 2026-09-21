//! Aeronet endpoint lifecycle bridge.
//!
//! [`EndpointAeronetPlugin`](crate::endpoint::EndpointAeronetPlugin) mirrors Aeronet endpoint state
//! onto the Lightyear entity that owns the socket. Concrete Aeronet-backed transports, such as
//! WebSocket, WebTransport and Steam, spawn an Aeronet server entity with
//! [`AeronetLinkOf`](crate::AeronetLinkOf) pointing at the Lightyear
//! [`Endpoint`](lightyear_link::endpoint::Endpoint). This module observes Aeronet open/close events
//! and keeps the Lightyear lifecycle markers in sync.
//!
//! Nothing here is tied to the Lightyear server role: Aeronet calls the accepting side of a
//! session a "server", but the Lightyear entity it mirrors may be any endpoint.

use alloc::{format, string::ToString};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;

use crate::AeronetLinkOf;
use aeronet_io::server::{CloseReason, Closed, Server, ServerEndpoint};
use lightyear_link::endpoint::EndpointLinkPlugin;
use lightyear_link::{Linked, Linking, UnlinkReason, Unlinked};
use tracing::trace;

/// Plugin that mirrors Aeronet endpoint state into Lightyear endpoint link state.
///
/// The plugin ensures [`EndpointLinkPlugin`] is installed, then observes Aeronet
/// [`ServerEndpoint`], [`Server`], and [`Closed`] events to insert [`Linking`], [`Linked`], and
/// [`Unlinked`] on the Lightyear endpoint entity.
pub struct EndpointAeronetPlugin;

impl EndpointAeronetPlugin {
    fn on_opening(
        trigger: On<Add<ServerEndpoint>>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(child_of.0)
        {
            trace!(
                "Aeronet endpoint opening for {:?}. Adding Linking",
                child_of.0
            );
            c.insert(Linking);
        }
    }

    fn on_opened(trigger: On<Add<Server>>, query: Query<&AeronetLinkOf>, mut commands: Commands) {
        if let Ok(child_of) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(child_of.0)
        {
            trace!(
                "Aeronet endpoint opened for {:?}. Adding Linked",
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
                "Aeronet endpoint closed for {:?}. Adding Unlinked",
                child_of.0
            );
            let reason = match &trigger.reason {
                CloseReason::ByUser(reason) => {
                    UnlinkReason::UserRequested((!reason.is_empty()).then(|| reason.to_string()))
                }
                CloseReason::ByError(err) => UnlinkReason::TransportError(format!("{err:?}")),
            };
            c.insert(Unlinked { reason });
        }
    }
}

impl Plugin for EndpointAeronetPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EndpointLinkPlugin>() {
            app.add_plugins(EndpointLinkPlugin);
        }
        app.add_observer(Self::on_opening);
        app.add_observer(Self::on_opened);
        app.add_observer(Self::on_closed);
    }
}
