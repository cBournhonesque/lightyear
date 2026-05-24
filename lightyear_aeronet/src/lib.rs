//! Shared adapter between Aeronet sessions and Lightyear [`Link`] entities.
//!
//! Aeronet owns concrete connection/session entities and packet queues. Lightyear owns
//! transport-neutral [`Link`] buffers and lifecycle markers. This crate bridges the two by keeping
//! an Aeronet session entity related to a Lightyear link entity through [`AeronetLink`] and
//! [`AeronetLinkOf`], copying Aeronet endpoint addresses onto the link, mirroring Aeronet session
//! state into [`Linked`], [`Linking`], and [`Unlinked`], and moving byte payloads between
//! [`aeronet_io::Session`] queues and [`Link`] buffers.
//!
//! Concrete Aeronet-backed transports such as `lightyear_websocket` and `lightyear_webtransport`
//! build on this crate. Server-specific lifecycle bridging lives in [`server`].
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

/// Server-side Aeronet lifecycle bridge for Lightyear server link entities.
pub mod server;

use crate::alloc::string::ToString;
use aeronet_io::connection::{Disconnect, DisconnectReason, Disconnected, LocalAddr, PeerAddr};
use aeronet_io::server::{Close, Server};
use aeronet_io::{IoSystems, Session, SessionEndpoint};
use alloc::format;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_reflect::Reflect;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSystems, LinkSystems, Linked, Linking, Unlink, UnlinkReason,
    Unlinked,
};
use tracing::trace;

/// Relationship target stored on a Lightyear [`Link`] entity for its Aeronet session entity.
///
/// The relationship points from an Aeronet session or server entity carrying [`AeronetLinkOf`] back
/// to the Lightyear entity that owns the transport-neutral [`Link`]. The plugin uses this mapping
/// to mirror lifecycle state and transfer queued payloads.
#[derive(Component, Reflect)]
#[relationship_target(relationship = AeronetLinkOf, linked_spawn)]
pub struct AeronetLink(#[relationship] Entity);

/// Relationship source stored on an Aeronet session or server entity.
///
/// The inner entity is the Lightyear entity that owns the corresponding [`Link`] or
/// [`lightyear_link::server::Server`]. Concrete Aeronet transports insert this component on their
/// Aeronet child entity after spawning it.
#[derive(Component, Reflect)]
#[relationship(relationship_target = AeronetLink)]
pub struct AeronetLinkOf(pub Entity);

/// Plugin that bridges Aeronet session state and queues into Lightyear links.
///
/// This plugin installs observers that:
/// - copy Aeronet endpoint [`LocalAddr`] and [`PeerAddr`] components onto the Lightyear link entity;
/// - mirror Aeronet connecting/connected/disconnected state into [`Linking`], [`Linked`], and
///   [`Unlinked`];
/// - translate [`Unlink`] into Aeronet [`Disconnect`] or server [`Close`] requests;
/// - move payloads between [`Session::recv`]/[`Session::send`] and [`Link::recv`]/[`Link::send`].
///
/// Concrete transports should add this plugin before scheduling their Aeronet open/connect systems.
pub struct AeronetPlugin;

impl AeronetPlugin {
    /// Copies [`LocalAddr`] from an Aeronet entity onto its Lightyear [`Link`] entity.
    fn on_local_addr_added(
        trigger: On<Add, (LocalAddr, AeronetLinkOf)>,
        query: Query<(&AeronetLinkOf, &LocalAddr)>,
        mut commands: Commands,
    ) {
        if let Ok((aeronet_link, local_addr)) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "LocalAddr added on Aeronet entity {:?}. Adding on Link entity {:?}",
                trigger.entity, aeronet_link.0
            );
            c.insert(LocalAddr(local_addr.0));
        }
    }

    /// Copies [`PeerAddr`] from an Aeronet entity onto its Lightyear [`Link`] entity.
    fn on_peer_addr_added(
        trigger: On<Add, (PeerAddr, AeronetLinkOf)>,
        query: Query<(&AeronetLinkOf, &PeerAddr)>,
        mut commands: Commands,
    ) {
        if let Ok((aeronet_link, peer_addr)) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "PeerAddr added on Aeronet entity {:?}. Adding on Link entity {:?}",
                trigger.entity, aeronet_link.0
            );
            c.insert(PeerAddr(peer_addr.0));
        }
    }

    fn on_connecting(
        trigger: On<Add, (SessionEndpoint, AeronetLinkOf)>,
        query: Query<&AeronetLinkOf, With<SessionEndpoint>>,
        linked_query: Query<(), With<Linked>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "SessionEndpoint added on AeronetLink {:?}. Adding Linking on Link entity {:?}",
                trigger.entity, aeronet_link.0
            );
            // If `Linked` is already inserted, we don't want to insert `Linking`
            // (sometimes, both `Linked` and `Linking` get inserted at the same frame).
            if !linked_query.contains(aeronet_link.0) {
                c.insert(Linking);
            }
        }
    }

    fn on_connected(
        trigger: On<Add, (Session, AeronetLinkOf)>,
        query: Query<&AeronetLinkOf, With<Session>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "Session added on AeronetLink {:?}. Adding Linked on Link entity {:?}",
                trigger.entity, aeronet_link.0
            );
            c.insert(Linked);
        }
    }

    fn on_disconnected(
        trigger: On<Disconnected>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_io) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(aeronet_io.0)
        {
            let reason = match &trigger.reason {
                DisconnectReason::ByUser(_) => UnlinkReason::ClientRequested,
                DisconnectReason::ByPeer(reason) => UnlinkReason::ByPeer(reason.to_string()),
                DisconnectReason::ByError(err) => UnlinkReason::TransportError(format!("{err:?}")),
            };
            trace!(
                "Disconnected (reason: {reason:?}) triggered added on AeronetLink {:?}. Adding Unlinked on Link entity {:?}",
                trigger.entity, aeronet_io.0
            );
            // we try insert, because the LinkOf entity might have been despawned already
            c.try_insert(Unlinked { reason });
        }
    }

    fn unlink(
        mut trigger: On<Unlink>,
        query: Query<&AeronetLink>,
        aeronet_query: Query<Has<Server>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.entity)
            // get the aeronet session entity
            && let Ok(is_server) = aeronet_query.get(aeronet_link.0)
        {
            let reason = core::mem::take(&mut trigger.reason);
            trace!(
                "Unlink triggered on Link entity {:?} (reason: {reason:?}). Closing/Disconnecting AeronetLink entity {:?}",
                trigger.entity, aeronet_link.0
            );
            if is_server {
                commands.trigger(Close::new(aeronet_link.0, reason.to_string()));
            } else {
                commands.trigger(Disconnect::new(aeronet_link.0, reason.to_string()));
            }
        }
    }

    fn receive(
        mut session_query: Query<(&mut Session, &AeronetLinkOf)>,
        mut link_query: Query<&mut Link, With<Linked>>,
    ) {
        session_query.iter_mut().for_each(|(mut session, parent)| {
            if let Ok(mut link) = link_query.get_mut(parent.get()) {
                trace!("Received {:?} packets", session.recv.len());
                session.recv.drain(..).for_each(|recv| {
                    #[cfg(feature = "test_utils")]
                    link.recv
                        .push(recv.payload, lightyear_core::time::Instant::now());
                    #[cfg(not(feature = "test_utils"))]
                    link.recv.push(recv.payload, recv.recv_at);
                });
            }
        });
    }

    fn send(
        mut session_query: Query<(&mut Session, &AeronetLinkOf)>,
        mut link_query: Query<&mut Link, With<Linked>>,
    ) {
        session_query.iter_mut().for_each(|(mut session, parent)| {
            if let Ok(mut link) = link_query.get_mut(parent.get()) {
                trace!("Sending {:?} packet", link.send.len());
                link.send.drain().for_each(|payload| {
                    session.send.push(payload);
                });
            }
        });
    }
}

impl Plugin for AeronetPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }

        app.add_observer(Self::on_local_addr_added);
        app.add_observer(Self::on_peer_addr_added);
        app.add_observer(Self::on_connecting);
        app.add_observer(Self::on_connected);
        app.add_observer(Self::on_disconnected);
        app.add_observer(Self::unlink);

        app.configure_sets(PreUpdate, LinkSystems::Receive.after(IoSystems::Poll));
        app.configure_sets(PostUpdate, LinkSystems::Send.before(IoSystems::Flush));
        app.add_systems(
            PreUpdate,
            Self::receive.in_set(LinkReceiveSystems::BufferToLink),
        );
        app.add_systems(PostUpdate, Self::send.in_set(LinkSystems::Send));
    }
}
