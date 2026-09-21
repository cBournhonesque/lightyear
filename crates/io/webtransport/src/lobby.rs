//! Glue between [`lightyear_p2p::Lobby`] and WebTransport links.
//!
//! Dial contexts carry a socket target and certificate hash independently of the peer's alias.
//! Every dial requires a nonempty native context. Both dialed and accepted links exchange the
//! existing lobby self-announcement before admission;
//! an accepted session's ephemeral address is never a public peer identity. The announcement
//! rejects identity conflicts; it does not authenticate the association between an alias and a
//! transport certificate.

use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use lightyear_aeronet::endpoint::EndpointAeronetPlugin;
use lightyear_connection::client::{Client, Connect, Connected, Disconnected};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId, peer_id_from_socket};
use lightyear_link::prelude::{Link, LinkOf, LinkStart, Linked, Unlinked};
use lightyear_p2p::{
    DialContext, DialPeer, Lobby, PendingLobbyPeer, begin_peer_resolution,
    resolve_endpoint_identity,
};
use tracing::{debug, warn};

use crate::client::WebTransportClientIo;
use crate::endpoint::{WebTransportEndpoint, WebTransportEndpointPlugin};
use crate::{WebTransportLocalAddr, WebTransportPeerAddr};

/// Dials WebTransport links for a [`Lobby`] through a [`WebTransportEndpoint`].
///
/// The endpoint publishes its bound socket and certificate hash independently of its opaque alias.
#[derive(Default)]
pub struct WebTransportLobbyPlugin;

impl Plugin for WebTransportLobbyPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EndpointAeronetPlugin>() {
            app.add_plugins(EndpointAeronetPlugin);
        }
        if !app.is_plugin_added::<WebTransportEndpointPlugin>() {
            app.add_plugins(WebTransportEndpointPlugin);
        }
        if !app.is_plugin_added::<crate::client::WebTransportClientPlugin>() {
            app.add_plugins(crate::client::WebTransportClientPlugin);
        }
        app.add_observer(on_endpoint_added);
        app.add_observer(on_endpoint_ready);
        app.add_observer(on_dial);
        app.add_observer(on_accepted_link);
        app.add_observer(on_accepted_identity);
    }
}

/// Encodes a WebTransport socket and SHA-256 certificate hash independently of its peer alias.
///
/// The versioned `WT` domain contains address family, network-order address and port, IPv6
/// flow info and scope ID when applicable, then the 32-byte hash. Its maximum size is 62 bytes.
pub fn dial_context(address: SocketAddr, certificate_digest: &[u8; 32]) -> DialContext {
    let mut bytes = Vec::with_capacity(62);
    bytes.extend_from_slice(b"WT\x01");
    match address {
        SocketAddr::V4(address) => {
            bytes.push(4);
            bytes.extend_from_slice(&address.ip().octets());
            bytes.extend_from_slice(&address.port().to_be_bytes());
        }
        SocketAddr::V6(address) => {
            bytes.push(6);
            bytes.extend_from_slice(&address.ip().octets());
            bytes.extend_from_slice(&address.port().to_be_bytes());
            bytes.extend_from_slice(&address.flowinfo().to_be_bytes());
            bytes.extend_from_slice(&address.scope_id().to_be_bytes());
        }
    }
    bytes.extend_from_slice(certificate_digest);
    DialContext::from(bytes)
}

fn decode_context(bytes: &[u8]) -> Option<(SocketAddr, &[u8])> {
    if bytes.get(..3)? != b"WT\x01" {
        return None;
    }
    match (bytes.get(3)?, bytes.len()) {
        (4, 42) => Some((
            SocketAddr::from((
                Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[4..8]).ok()?),
                u16::from_be_bytes(bytes[8..10].try_into().ok()?),
            )),
            &bytes[10..],
        )),
        (6, 62) => Some((
            SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[4..20]).ok()?),
                u16::from_be_bytes(bytes[20..22].try_into().ok()?),
                u32::from_be_bytes(bytes[22..26].try_into().ok()?),
                u32::from_be_bytes(bytes[26..30].try_into().ok()?),
            )),
            &bytes[30..],
        )),
        _ => None,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Endpoint whose accepted links must finish lobby identity setup before admission.
#[derive(Component)]
pub(crate) struct LobbyEndpoint;

/// Cache the hash until the endpoint's native address changes, rather than hashing per frame.
#[derive(Component)]
struct PublishedContext(SocketAddr);

fn on_endpoint_added(
    trigger: On<Add, WebTransportEndpoint>,
    servers: Query<(), With<lightyear_link::server::Server>>,
    mut commands: Commands,
) {
    if servers.contains(trigger.entity) {
        return;
    }
    commands.entity(trigger.entity).insert(LobbyEndpoint);
    commands.trigger(LinkStart {
        entity: trigger.entity,
    });
}

/// Resolves identity once; a later address update changes only the dial context.
fn prepare_endpoint(world: &mut World, endpoint: Entity) -> Option<PeerId> {
    let address = world.get::<WebTransportLocalAddr>(endpoint)?.0;
    let candidate = world.get::<LocalId>(endpoint).map_or_else(
        || peer_id_from_socket(b"webtransport", address),
        |local| local.0,
    );
    let local = resolve_endpoint_identity(world, endpoint, candidate)?;
    if world
        .get::<PublishedContext>(endpoint)
        .is_none_or(|published| published.0 != address)
    {
        let endpoint_io = world.get::<WebTransportEndpoint>(endpoint)?;
        let chain = endpoint_io.certificate.certificate_chain();
        let certificate = chain.as_slice().first()?;
        let hash = certificate.hash();
        let digest: &[u8; 32] = hash.as_ref().try_into().ok()?;
        let context = dial_context(address, digest);
        world.resource_mut::<Lobby>().set_dial_context(context);
        world.entity_mut(endpoint).insert(PublishedContext(address));
    }
    Some(local)
}

fn on_endpoint_ready(
    trigger: On<Insert, (WebTransportLocalAddr, LobbyEndpoint)>,
    endpoints: Query<(), (With<WebTransportEndpoint>, With<LobbyEndpoint>)>,
    mut commands: Commands,
) {
    if endpoints.contains(trigger.entity) {
        let endpoint = trigger.entity;
        commands.queue(move |world: &mut World| {
            prepare_endpoint(world, endpoint);
        });
    }
}

fn on_dial(
    trigger: On<DialPeer>,
    endpoints: Query<Entity, (With<WebTransportEndpoint>, With<LobbyEndpoint>)>,
    mut commands: Commands,
) {
    let peer = trigger.peer;
    let target =
        decode_context(&trigger.context).map(|(address, digest)| (address, to_hex(digest)));
    let Some((address, certificate_digest)) = target else {
        warn!(?peer, "no usable WebTransport dial context");
        return;
    };
    let Ok(endpoint) = endpoints.single() else {
        warn!(
            ?peer,
            "a single lobby WebTransportEndpoint is required to dial"
        );
        return;
    };
    commands.queue(move |world: &mut World| {
        if prepare_endpoint(world, endpoint).is_none() {
            return;
        }
        let mut existing = world.query_filtered::<(
            Entity,
            Option<&RemoteId>,
            Option<&PendingLobbyPeer>,
            Has<Disconnected>,
            Has<Unlinked>,
            Has<WebTransportClientIo>,
            Option<&LinkOf>,
        ), (With<P2P>, With<Client>)>();
        let mut obsolete = Vec::new();
        for (entity, remote, pending, disconnected, unlinked, dialed, owner) in existing.iter(world)
        {
            if remote
                .map(|remote| remote.0)
                .or_else(|| pending.and_then(PendingLobbyPeer::expected_remote))
                != Some(peer)
            {
                continue;
            }
            if !disconnected && !unlinked {
                return;
            }
            if dialed || owner.is_some_and(|owner| owner.endpoint == endpoint) {
                obsolete.push(entity);
            }
        }
        for entity in obsolete {
            world.despawn(entity);
        }
        debug!(?peer, %address, "lobby dialed a WebTransport peer");
        // Install P2P before any transport callback can invoke default C/S admission.
        let link = world.spawn((P2P::default(), Client, Link::default())).id();
        if !begin_peer_resolution(world, link, endpoint, Some(peer)) {
            world.despawn(link);
            return;
        }
        world
            .entity_mut(link)
            .insert(WebTransportClientIo::from_addr(address, certificate_digest));
        world.trigger(Connect { entity: link });
    });
}

/// Retires obsolete transport-owned links after an inbound replacement is admitted.
fn on_accepted_identity(
    trigger: On<Insert, (RemoteId, Connected)>,
    accepted: Query<(&RemoteId, &LinkOf), (With<P2P>, With<Connected>)>,
    endpoints: Query<(), With<WebTransportEndpoint>>,
    obsolete: Query<
        (
            Entity,
            &RemoteId,
            Has<WebTransportClientIo>,
            Option<&LinkOf>,
        ),
        (
            With<P2P>,
            With<Client>,
            Or<(With<Disconnected>, With<Unlinked>)>,
        ),
    >,
    mut commands: Commands,
) {
    let Ok((remote, owner)) = accepted.get(trigger.entity) else {
        return;
    };
    if !endpoints.contains(owner.endpoint) {
        return;
    }
    for (entity, old_remote, dialed, link_of) in &obsolete {
        if entity != trigger.entity
            && old_remote == remote
            && (dialed || link_of.is_some_and(|link| link.endpoint == owner.endpoint))
        {
            commands.entity(entity).despawn();
        }
    }
}

fn prepare_accepted(world: &mut World, entity: Entity, endpoint: Entity) {
    if world.get::<Connected>(entity).is_some() {
        return;
    }
    // Mark ownership even when the peer address or endpoint bind callback is delayed.
    world.entity_mut(entity).insert((P2P::default(), Client));
    if world.get::<Linked>(entity).is_none()
        || world.get::<WebTransportPeerAddr>(entity).is_none()
        || prepare_endpoint(world, endpoint).is_none()
    {
        return;
    }
    begin_peer_resolution(world, entity, endpoint, None);
}

fn on_accepted_link(
    trigger: On<
        Add,
        (
            WebTransportPeerAddr,
            WebTransportLocalAddr,
            LinkOf,
            Linked,
            LobbyEndpoint,
        ),
    >,
    links: Query<(Entity, &LinkOf)>,
    endpoints: Query<(), With<LobbyEndpoint>>,
    mut commands: Commands,
) {
    let mut prepare = |entity, endpoint| {
        commands.queue(move |world: &mut World| prepare_accepted(world, entity, endpoint));
    };
    if endpoints.contains(trigger.entity) {
        for (entity, owner) in &links {
            if owner.endpoint == trigger.entity {
                prepare(entity, owner.endpoint);
            }
        }
    } else if let Ok((entity, owner)) = links.get(trigger.entity) {
        if endpoints.contains(owner.endpoint) {
            prepare(entity, owner.endpoint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_connection::network_topology::NetworkingMetadata;
    use lightyear_p2p::LobbyPlugin;

    #[cfg(feature = "self-signed")]
    #[test]
    fn repeated_opted_out_dials_leave_no_orphan_links() {
        use lightyear_connection::admission::NoDefaultConnection;

        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        app.add_plugins(LobbyPlugin::default());
        app.add_observer(on_dial);
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 4100));
        let endpoint = app
            .world_mut()
            .spawn((
                WebTransportEndpoint {
                    bind_addr: address,
                    certificate: aeronet_webtransport::wtransport::Identity::self_signed([
                        "localhost",
                    ])
                    .unwrap(),
                },
                WebTransportLocalAddr(address),
                LobbyEndpoint,
                LocalId(PeerId(91)),
                NoDefaultConnection,
            ))
            .id();
        let dial = DialPeer {
            peer: PeerId(92),
            context: dial_context(SocketAddr::from((Ipv4Addr::LOCALHOST, 4101)), &[37; 32]),
        };
        for _ in 0..3 {
            app.world_mut().trigger(dial.clone());
            app.world_mut().flush();
            let world = app.world_mut();
            assert_eq!(
                world
                    .query_filtered::<Entity, (With<Link>, Without<WebTransportEndpoint>)>()
                    .iter(world)
                    .count(),
                0
            );
        }
        app.world_mut()
            .entity_mut(endpoint)
            .remove::<NoDefaultConnection>();
        app.world_mut().trigger(dial);
        app.world_mut().flush();
        let world = app.world_mut();
        let mut pending = world.query::<(&PendingLobbyPeer, &WebTransportClientIo)>();
        let (pending, _) = pending.single(world).unwrap();
        assert_eq!(pending.expected_remote(), Some(PeerId(92)));
    }

    #[test]
    fn socket_context_preserves_certificate_and_ipv6_scope_and_rejects_truncation() {
        let address = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 4100, 7, 9));
        let digest = [37; 32];
        let context = dial_context(address, &digest);
        assert_eq!(decode_context(&context), Some((address, digest.as_slice())));
        assert!(context.len() <= lightyear_p2p::MAX_DIAL_CONTEXT);
        for end in 0..context.len() {
            assert!(decode_context(&context[..end]).is_none());
        }
        let mut invalid = context.to_vec();
        invalid[0] = b'S';
        assert!(decode_context(&invalid).is_none());
        invalid[0] = b'W';
        invalid.push(0);
        assert!(decode_context(&invalid).is_none());
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 4101));
        assert_eq!(
            decode_context(&dial_context(address, &digest)),
            Some((address, digest.as_slice())),
        );
    }

    #[cfg(feature = "self-signed")]
    #[test]
    fn accepted_link_waits_for_endpoint_facts_without_publishing_ephemeral_ids() {
        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        app.add_plugins(LobbyPlugin::default());
        app.add_observer(on_endpoint_ready);
        app.add_observer(on_accepted_link);
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 4100));
        let alias = PeerId(91);
        let endpoint = app
            .world_mut()
            .spawn((
                WebTransportEndpoint {
                    bind_addr: address,
                    certificate: aeronet_webtransport::wtransport::Identity::self_signed([
                        "localhost",
                    ])
                    .unwrap(),
                },
                LobbyEndpoint,
                LocalId(alias),
            ))
            .id();
        let link = app
            .world_mut()
            .spawn((LinkOf { endpoint }, Link::default(), Linked))
            .id();
        assert!(app.world().get::<P2P>(link).is_some());
        app.world_mut()
            .entity_mut(link)
            .insert(WebTransportPeerAddr(SocketAddr::from((
                Ipv4Addr::LOCALHOST,
                53000,
            ))));
        assert!(app.world().get::<PendingLobbyPeer>(link).is_none());
        assert!(app.world().get::<RemoteId>(link).is_none());
        app.world_mut()
            .entity_mut(endpoint)
            .insert(WebTransportLocalAddr(address));
        assert!(app.world().get::<PendingLobbyPeer>(link).is_some());
        assert!(app.world().get::<Connected>(link).is_none());
        assert!(app.world().get::<RemoteId>(link).is_none());
        assert!(app.world().get::<LocalId>(link).is_none());
        assert_eq!(app.world().get::<LocalId>(endpoint), Some(&LocalId(alias)));
        assert_eq!(app.world().resource::<Lobby>().local(), Some(alias));
        let changed = SocketAddr::from((Ipv4Addr::LOCALHOST, 4102));
        app.world_mut()
            .entity_mut(endpoint)
            .insert(WebTransportLocalAddr(changed));
        assert_eq!(app.world().resource::<Lobby>().local(), Some(alias));
        assert_eq!(app.world().get::<LocalId>(endpoint), Some(&LocalId(alias)));
        assert_eq!(
            decode_context(app.world().resource::<Lobby>().local_dial_context())
                .map(|(address, _)| address),
            Some(changed),
        );
        assert!(app.world().get::<PendingLobbyPeer>(link).is_some());
        assert!(app.world().get::<RemoteId>(link).is_none());
    }
}
