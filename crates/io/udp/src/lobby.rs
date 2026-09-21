//! UDP routing glue for [`lightyear_p2p::Lobby`].
//!
//! A peer owns one [`UdpEndpoint`] socket. Child links route through that socket, while the
//! endpoint's resolved `LocalId` is inherited by every child. The default identity comes from
//! the first bound address; an explicit endpoint identity is preserved.
//!
//! Add this plugin alongside `LobbyPlugin` and the normal P2P messaging/session plugins.
//! Spawn `UdpEndpoint::new(bind)` and optionally configure its `LocalId` before adding the IO.
//! Port-zero binds publish [`crate::UdpLocalAddr`]. Discovery always requires [`dial_context`]:
//! socket routing belongs in discovery context, never in the peer alias.
//!
//! A UDP link becomes [`Linked`] immediately, but a lobby link is admitted only after receiving
//! the peer's existing identity announcement on the reliable P2P channel. No new raw handshake
//! or identity timer is introduced; non-lobby default UDP admission is unchanged.
//! Unanswered peers cannot join a deterministic session. Foreign-lobby peers remain separate
//! from lobby membership even when their connection identity is resolved.

use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_core::id::{LocalId, PeerId, peer_id_from_socket};
use lightyear_link::prelude::{Link, LinkOf, LinkStart, Linked};
use lightyear_p2p::{
    DialContext, DialPeer, Lobby, begin_peer_resolution, resolve_endpoint_identity,
};
use tracing::{debug, warn};

use crate::{UdpLocalAddr, UdpPeerAddr};

use crate::endpoint::{UdpEndpoint, UdpEndpointPlugin, UdpLinkOfIO};

/// Dials UDP Links for a [`Lobby`](lightyear_p2p::Lobby) through a [`UdpEndpoint`].
pub struct UdpLobbyPlugin;

/// Identifies endpoints whose children need P2P roles before admission.
#[derive(Component, Default)]
pub(crate) struct UdpLobbyEndpoint;

/// Dial intent, not an admitted remote identity.
#[derive(Component)]
pub(crate) struct UdpExpectedPeer(pub PeerId);

/// Encodes a UDP target independently of its routing alias.
pub fn dial_context(address: core::net::SocketAddr) -> DialContext {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(b"UDP\x01");
    match address {
        core::net::SocketAddr::V4(address) => {
            bytes.push(4);
            bytes.extend_from_slice(&address.ip().octets());
            bytes.extend_from_slice(&address.port().to_be_bytes());
        }
        core::net::SocketAddr::V6(address) => {
            bytes.push(6);
            bytes.extend_from_slice(&address.ip().octets());
            bytes.extend_from_slice(&address.port().to_be_bytes());
            bytes.extend_from_slice(&address.flowinfo().to_be_bytes());
            bytes.extend_from_slice(&address.scope_id().to_be_bytes());
        }
    }
    DialContext::from(bytes)
}

fn context_address(context: &[u8]) -> Option<core::net::SocketAddr> {
    let bytes = context.strip_prefix(b"UDP\x01")?;
    match (bytes.first().copied()?, bytes.len()) {
        (4, 7) => Some(core::net::SocketAddr::new(
            core::net::Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[1..5]).ok()?).into(),
            u16::from_be_bytes(bytes[5..7].try_into().ok()?),
        )),
        (6, 27) => Some(core::net::SocketAddr::V6(core::net::SocketAddrV6::new(
            core::net::Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[1..17]).ok()?),
            u16::from_be_bytes(bytes[17..19].try_into().ok()?),
            u32::from_be_bytes(bytes[19..23].try_into().ok()?),
            u32::from_be_bytes(bytes[23..27].try_into().ok()?),
        ))),
        _ => None,
    }
}

impl Plugin for UdpLobbyPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<UdpEndpoint, UdpLobbyEndpoint>();
        if !app.is_plugin_added::<UdpEndpointPlugin>() {
            app.add_plugins(UdpEndpointPlugin);
        }
        app.add_observer(on_endpoint_added);
        app.add_observer(on_endpoint_bound);
        app.add_observer(on_dial);
    }
}

/// Binds the endpoint's socket as soon as it exists.
fn on_endpoint_added(trigger: On<Add, UdpEndpoint>, mut commands: Commands) {
    commands.trigger(LinkStart {
        entity: trigger.entity,
    });
}

fn on_endpoint_bound(trigger: On<Insert, UdpLocalAddr>, mut commands: Commands) {
    let endpoint = trigger.entity;
    commands.queue(move |world: &mut World| {
        if world.get::<UdpLobbyEndpoint>(endpoint).is_none() {
            return;
        }
        let Some(address) = world.get::<UdpLocalAddr>(endpoint).map(|address| address.0) else {
            return;
        };
        let candidate = world
            .get::<LocalId>(endpoint)
            .map_or_else(|| peer_id_from_socket(b"udp", address), |local| local.0);
        if resolve_endpoint_identity(world, endpoint, candidate).is_none() {
            return;
        }
        world
            .resource_mut::<Lobby>()
            .set_dial_context(dial_context(address));
        let children: Vec<_> = world
            .query_filtered::<(Entity, &LinkOf), With<UdpLinkOfIO>>()
            .iter(world)
            .filter_map(|(entity, link)| (link.endpoint == endpoint).then_some(entity))
            .collect();
        for entity in children {
            let expected = world
                .get::<UdpExpectedPeer>(entity)
                .map(|expected| expected.0);
            begin_peer_resolution(world, entity, endpoint, expected);
        }
    });
}
/// Opens a Link to the peer the lobby asked for.
///
/// The native address comes from discovery context. Register the child before a response can
/// arrive, so replies use this pending link rather than creating a duplicate.
fn on_dial(
    trigger: On<DialPeer>,
    endpoints: Query<Entity, With<UdpEndpoint>>,
    peer_links: Query<(&LinkOf, &UdpPeerAddr), With<UdpLinkOfIO>>,
    mut ios: Query<&mut UdpEndpoint>,
    mut commands: Commands,
) {
    let peer = trigger.peer;
    let address = context_address(&trigger.context);
    let Some(address) = address else {
        warn!(?peer, "UDP discovery needs a valid UDP dial context");
        return;
    };

    let Ok(endpoint) = endpoints.single() else {
        warn!(
            ?peer,
            "no UdpEndpoint exists, so no Link can be dialed; spawn one with UdpEndpoint::new"
        );
        return;
    };

    // Reconnecting to a peer we already have a link to would put two links on one address, and the
    // endpoint would deliver the peer's datagrams to only one of them.
    if peer_links
        .iter()
        .any(|(link_of, peer_addr)| link_of.endpoint == endpoint && peer_addr.0 == address)
    {
        debug!(
            ?peer,
            "a Link to this peer already exists; not dialing again"
        );
        return;
    }

    let link = commands
        .spawn((
            UdpExpectedPeer(peer),
            LinkOf { endpoint },
            Link::default(),
            // The endpoint's socket is already bound, so the link can send immediately.
            Linked,
            UdpPeerAddr(address),
            UdpLinkOfIO,
        ))
        .id();
    if let Ok(mut io) = ios.get_mut(endpoint) {
        io.register_link(address, link);
    }
    debug!(?peer, %address, ?link, "lobby dialed a UDP peer");
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::{Ipv4Addr, SocketAddr};
    use lightyear_p2p::LobbyPlugin;

    #[test]
    fn native_context_updates_preserve_chosen_and_default_endpoint_ids() {
        let original = SocketAddr::from((Ipv4Addr::LOCALHOST, 4100));
        let changed = SocketAddr::from((Ipv4Addr::LOCALHOST, 4101));
        for requested in [None, Some(PeerId(91))] {
            let mut app = App::new();
            app.add_plugins(LobbyPlugin::default());
            app.add_observer(on_endpoint_bound);
            let endpoint = app
                .world_mut()
                .spawn((UdpEndpoint::new(original), UdpLobbyEndpoint))
                .id();
            if let Some(alias) = requested {
                app.world_mut().entity_mut(endpoint).insert(LocalId(alias));
            }
            app.world_mut()
                .entity_mut(endpoint)
                .insert(UdpLocalAddr(original));
            let alias = requested.unwrap_or_else(|| peer_id_from_socket(b"udp", original));
            assert_eq!(app.world().resource::<Lobby>().local(), Some(alias));
            assert_eq!(
                context_address(app.world().resource::<Lobby>().local_dial_context()),
                Some(original),
            );
            app.world_mut()
                .entity_mut(endpoint)
                .insert(UdpLocalAddr(changed));
            assert_eq!(app.world().resource::<Lobby>().local(), Some(alias));
            assert_eq!(app.world().get::<LocalId>(endpoint), Some(&LocalId(alias)));
            assert_eq!(
                context_address(app.world().resource::<Lobby>().local_dial_context()),
                Some(changed),
            );
        }
    }
}
