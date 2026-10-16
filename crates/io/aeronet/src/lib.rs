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
//! build on this crate. Endpoint lifecycle bridging lives in [`endpoint`].
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

/// Aeronet endpoint lifecycle bridge for Lightyear endpoint link entities.
pub mod endpoint;

use aeronet_io::connection::{Disconnect, DisconnectReason, Disconnected, LocalAddr, PeerAddr};
use aeronet_io::server::{Close, Server};
use aeronet_io::{IoSystems, Session, SessionEndpoint};
use alloc::{format, string::ToString};
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_reflect::Reflect;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSystems, LinkSystems, Linked, Linking, Unlink, UnlinkReason,
    Unlinked, recv_payload_from_bytes,
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
/// [`lightyear_link::endpoint::Endpoint`]. Concrete Aeronet transports insert this component on
/// their Aeronet child entity after spawning it.
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
        query: Query<(&AeronetLinkOf, &Session)>,
        mut link_query: Query<&mut Link>,
        mut commands: Commands,
    ) {
        if let Ok((aeronet_link, session)) = query.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "Session added on AeronetLink {:?}. Adding Linked on Link entity {:?}",
                trigger.entity, aeronet_link.0
            );
            if let Ok(mut link) = link_query.get_mut(aeronet_link.0) {
                // Aeronet drops packets larger than the session's MTU. Lower the link's minimum
                // before it links, since that's what the transport sizes fragments from.
                link.lower_min_mtu(session.min_mtu());
                Self::fit_mtu(&mut link, session);
            }
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
                DisconnectReason::ByUser(reason) => {
                    UnlinkReason::UserRequested((!reason.is_empty()).then(|| reason.to_string()))
                }
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
            let reason = core::mem::take(&mut trigger.reason).to_string();
            trace!(
                "Unlink triggered on Link entity {:?} (reason: {reason:?}). Closing/Disconnecting AeronetLink entity {:?}",
                trigger.entity, aeronet_link.0
            );
            if is_server {
                commands.trigger(Close::new(aeronet_link.0, reason));
            } else {
                commands.trigger(Disconnect::new(aeronet_link.0, reason));
            }
        }
    }

    fn fit_mtu(link: &mut Link, session: &Session) {
        link.set_mtu(session.mtu())
            .expect("the link's minimum MTU is lowered to the session's on connect");
    }

    fn receive(
        mut session_query: Query<(&mut Session, &AeronetLinkOf)>,
        mut link_query: Query<&mut Link, With<Linked>>,
    ) {
        session_query.iter_mut().for_each(|(mut session, parent)| {
            if let Ok(mut link) = link_query.get_mut(parent.get()) {
                Self::fit_mtu(&mut link, &session);
                trace!("Received {:?} packets", session.recv.len());
                session.recv.drain(..).for_each(|recv| {
                    #[cfg(feature = "test_utils")]
                    link.recv.push(
                        recv_payload_from_bytes(recv.payload),
                        lightyear_core::time::Instant::now(),
                    );
                    #[cfg(not(feature = "test_utils"))]
                    link.recv
                        .push(recv_payload_from_bytes(recv.payload), recv.recv_at);
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
                let mtu = session.mtu();
                link.send.drain().for_each(|payload| {
                    debug_assert!(payload.len() <= mtu, "packet larger than the session's MTU");
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

#[cfg(test)]
mod tests {
    use super::*;
    use aeronet_io::packet::IP_MTU;
    use lightyear_link::{DEFAULT_MTU, LinkMtu};
    use std::time::Instant;

    #[test]
    fn link_mtu_fits_session_mtu() {
        // QUIC datagram room on a path of 1200 and of 1452 bytes (quinn's discovery ceiling):
        // minus 29 for the short header with 8-byte connection ids, 9 for the DATAGRAM frame and
        // 1 for the WebTransport session id.
        let min_path = 1200 - 29 - 9 - 1;
        let discovered = 1452 - 29 - 9 - 1;
        let steam = 512 * 1024;
        // (configured link MTU, session min MTU, session MTU each frame)
        //   -> (link min MTU, link MTU each frame)
        let cases = [
            (
                DEFAULT_MTU,
                IP_MTU,
                [IP_MTU, min_path, discovered],
                IP_MTU,
                [IP_MTU, min_path, DEFAULT_MTU],
            ),
            (
                1452,
                IP_MTU,
                [IP_MTU, min_path, discovered],
                IP_MTU,
                [IP_MTU, min_path, discovered],
            ),
            (
                DEFAULT_MTU,
                steam,
                [steam; 3],
                DEFAULT_MTU,
                [DEFAULT_MTU; 3],
            ),
        ];
        for (configured_mtu, session_min_mtu, session_mtus, link_min_mtu, link_mtus) in cases {
            let mut app = App::new();
            app.add_plugins(AeronetPlugin);
            let link = app
                .world_mut()
                .spawn(Link::default().with_mtu(LinkMtu::new(configured_mtu)))
                .id();
            let session = app
                .world_mut()
                .spawn((
                    AeronetLinkOf(link),
                    Session::new(Instant::now(), session_min_mtu),
                ))
                .id();
            for (session_mtu, link_mtu) in session_mtus.into_iter().zip(link_mtus) {
                app.world_mut()
                    .get_mut::<Session>(session)
                    .unwrap()
                    .set_mtu(session_mtu)
                    .unwrap();
                app.update();
                let link = app.world().entity(link);
                assert!(link.contains::<Linked>());
                let link = link.get::<Link>().unwrap();
                assert_eq!(
                    (link.min_mtu(), link.mtu()),
                    (link_min_mtu, link_mtu),
                    "configured MTU {configured_mtu}, session min MTU {session_min_mtu}, \
                     session MTU {session_mtu}"
                );
            }
        }
    }
}
