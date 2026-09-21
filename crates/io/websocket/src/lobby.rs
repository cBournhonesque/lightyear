//! Glue between [`lightyear_p2p::Lobby`] and WebSocket links.
//!
//! Dial contexts carry a socket target and scheme independently of the peer's stable alias.
//! An empty context retains the `PeerId::Raw(socket)` bootstrap convenience. Both dialed and
//! accepted links exchange the existing lobby self-announcement before connection admission;
//! accepted socket addresses are never published as provisional peer identities.
//! This detects identity conflicts, not authenticated endpoint ownership.

use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use lightyear_aeronet::endpoint::EndpointAeronetPlugin;
use lightyear_connection::client::{Client, Connect, Connected, Disconnected};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::prelude::{Link, LinkOf, LinkStart, Linked, Unlinked};
use lightyear_p2p::{
    DialContext, DialPeer, Lobby, PendingLobbyPeer, begin_peer_resolution,
    resolve_endpoint_identity,
};
use tracing::{debug, warn};

use crate::{WebSocketLocalAddr, WebSocketPeerAddr};
use aeronet_websocket::client::ClientConfig;

use crate::client::{WebSocketClientIo, WebSocketScheme};
use crate::endpoint::{WebSocketEndpoint, WebSocketEndpointPlugin};

/// Dials WebSocket links for a [`Lobby`] through a [`WebSocketEndpoint`].
///
/// The scheme describes this endpoint and is also used for context-free bootstrap dials.
/// Discovered peers supply their own scheme in their transport dial context.
pub struct WebSocketLobbyPlugin {
    config: ClientConfig,
    scheme: WebSocketScheme,
}

impl WebSocketLobbyPlugin {
    /// Uses `config` for clients and advertises this endpoint with `scheme`.
    pub fn new(config: ClientConfig, scheme: WebSocketScheme) -> Self {
        Self { config, scheme }
    }
}

impl Default for WebSocketLobbyPlugin {
    fn default() -> Self {
        Self::new(ClientConfig::default(), WebSocketScheme::Plain)
    }
}

#[derive(Resource)]
struct DialConfig {
    config: ClientConfig,
    scheme: WebSocketScheme,
}

impl Plugin for WebSocketLobbyPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EndpointAeronetPlugin>() {
            app.add_plugins(EndpointAeronetPlugin);
        }
        if !app.is_plugin_added::<WebSocketEndpointPlugin>() {
            app.add_plugins(WebSocketEndpointPlugin);
        }
        if !app.is_plugin_added::<crate::client::WebSocketClientPlugin>() {
            app.add_plugins(crate::client::WebSocketClientPlugin);
        }
        app.insert_resource(DialConfig {
            config: self.config.clone(),
            scheme: self.scheme,
        });
        app.add_observer(on_endpoint_added);
        app.add_observer(on_endpoint_ready);
        app.add_observer(on_dial);
        app.add_observer(on_accepted_link);
        app.add_observer(on_accepted_identity);
    }
}

/// Encodes a WebSocket socket target independently of its chosen peer alias.
///
/// The versioned `WS` domain contains scheme, address family, network-order address and port,
/// and (for IPv6) flow info and scope ID. Its maximum size is 31 bytes.
pub fn dial_context(address: SocketAddr, scheme: WebSocketScheme) -> DialContext {
    let mut bytes = Vec::with_capacity(31);
    bytes.extend_from_slice(b"WS\x01");
    bytes.push(match scheme {
        WebSocketScheme::Plain => 0,
        WebSocketScheme::Secure => 1,
    });
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
    DialContext::from(bytes)
}

fn decode_context(bytes: &[u8]) -> Option<(SocketAddr, WebSocketScheme)> {
    if bytes.get(..3)? != b"WS\x01" {
        return None;
    }
    let scheme = match bytes.get(3)? {
        0 => WebSocketScheme::Plain,
        1 => WebSocketScheme::Secure,
        _ => return None,
    };
    let address = match (bytes.get(4)?, bytes.len()) {
        (4, 11) => SocketAddr::from((
            Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[5..9]).ok()?),
            u16::from_be_bytes(bytes[9..11].try_into().ok()?),
        )),
        (6, 31) => SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[5..21]).ok()?),
            u16::from_be_bytes(bytes[21..23].try_into().ok()?),
            u32::from_be_bytes(bytes[23..27].try_into().ok()?),
            u32::from_be_bytes(bytes[27..31].try_into().ok()?),
        )),
        _ => return None,
    };
    Some((address, scheme))
}

/// Endpoint whose accepted links must finish lobby identity setup before admission.
#[derive(Component)]
pub(crate) struct LobbyEndpoint;

#[derive(Component)]
struct PublishedContext(SocketAddr);

fn on_endpoint_added(
    trigger: On<Add, WebSocketEndpoint>,
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
    let address = world.get::<WebSocketLocalAddr>(endpoint)?.0;
    let candidate = world
        .get::<LocalId>(endpoint)
        .map_or(PeerId::Raw(address), |local| local.0);
    let local = resolve_endpoint_identity(world, endpoint, candidate)?;
    if world
        .get::<PublishedContext>(endpoint)
        .is_none_or(|published| published.0 != address)
    {
        let scheme = world.resource::<DialConfig>().scheme;
        world
            .resource_mut::<Lobby>()
            .set_dial_context(dial_context(address, scheme));
        world.entity_mut(endpoint).insert(PublishedContext(address));
    }
    Some(local)
}

fn on_endpoint_ready(
    trigger: On<Insert, (WebSocketLocalAddr, LobbyEndpoint)>,
    endpoints: Query<(), (With<WebSocketEndpoint>, With<LobbyEndpoint>)>,
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
    endpoints: Query<Entity, (With<WebSocketEndpoint>, With<LobbyEndpoint>)>,
    config: Res<DialConfig>,
    mut commands: Commands,
) {
    let peer = trigger.peer;
    let target = if trigger.context.is_empty() {
        match peer {
            PeerId::Raw(address) => Some((address, config.scheme)),
            _ => None,
        }
    } else {
        decode_context(&trigger.context)
    };
    let Some((address, scheme)) = target else {
        warn!(
            ?peer,
            "no usable WebSocket dial context or bootstrap socket"
        );
        return;
    };
    let Ok(endpoint) = endpoints.single() else {
        warn!(
            ?peer,
            "a single lobby WebSocketEndpoint is required to dial"
        );
        return;
    };
    let config = config.config.clone();
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
            Has<WebSocketClientIo>,
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
        debug!(?peer, %address, "lobby dialed a WebSocket peer");
        // Install P2P before any transport callback can invoke default C/S admission.
        let link = world.spawn((P2P::default(), Client, Link::default())).id();
        if !begin_peer_resolution(world, link, endpoint, Some(peer)) {
            world.despawn(link);
            return;
        }
        world
            .entity_mut(link)
            .insert(WebSocketClientIo::from_addr(config, scheme, address));
        world.trigger(Connect { entity: link });
    });
}

/// Retires obsolete transport-owned links after an inbound replacement is admitted.
fn on_accepted_identity(
    trigger: On<Insert, (RemoteId, Connected)>,
    accepted: Query<(&RemoteId, &LinkOf), (With<P2P>, With<Connected>)>,
    endpoints: Query<(), With<WebSocketEndpoint>>,
    obsolete: Query<
        (Entity, &RemoteId, Has<WebSocketClientIo>, Option<&LinkOf>),
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
        || world.get::<WebSocketPeerAddr>(entity).is_none()
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
            WebSocketPeerAddr,
            WebSocketLocalAddr,
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

    #[test]
    fn repeated_opted_out_dials_leave_no_orphan_links() {
        use lightyear_connection::admission::NoDefaultConnection;

        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        app.add_plugins(LobbyPlugin::default());
        app.insert_resource(DialConfig {
            config: ClientConfig::default(),
            scheme: WebSocketScheme::Plain,
        });
        app.add_observer(on_dial);
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 4100));
        let endpoint = app
            .world_mut()
            .spawn((
                WebSocketEndpoint {
                    config: aeronet_websocket::server::ServerConfig::builder()
                        .with_bind_address(address)
                        .with_no_encryption(),
                },
                WebSocketLocalAddr(address),
                LobbyEndpoint,
                LocalId(PeerId::Local(91)),
                NoDefaultConnection,
            ))
            .id();
        let dial = DialPeer {
            peer: PeerId::Local(92),
            context: dial_context(
                SocketAddr::from((Ipv4Addr::LOCALHOST, 4101)),
                WebSocketScheme::Plain,
            ),
        };
        for _ in 0..3 {
            app.world_mut().trigger(dial.clone());
            app.world_mut().flush();
            let world = app.world_mut();
            assert_eq!(
                world
                    .query_filtered::<Entity, (With<Link>, Without<WebSocketEndpoint>)>()
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
        let mut pending = world.query::<(&PendingLobbyPeer, &WebSocketClientIo)>();
        let (pending, _) = pending.single(world).unwrap();
        assert_eq!(pending.expected_remote(), Some(PeerId::Local(92)));
    }

    #[test]
    fn socket_context_preserves_scheme_and_ipv6_scope_and_rejects_truncation() {
        let address = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 4100, 7, 9));
        let context = dial_context(address, WebSocketScheme::Secure);
        assert_eq!(
            decode_context(&context),
            Some((address, WebSocketScheme::Secure))
        );
        assert!(context.len() <= lightyear_p2p::MAX_DIAL_CONTEXT);
        for end in 0..context.len() {
            assert!(decode_context(&context[..end]).is_none());
        }
        let mut invalid = context.to_vec();
        invalid[3] = 2;
        assert!(decode_context(&invalid).is_none());
        invalid[3] = 1;
        invalid.push(0);
        assert!(decode_context(&invalid).is_none());
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 4101));
        assert_eq!(
            decode_context(&dial_context(address, WebSocketScheme::Plain)),
            Some((address, WebSocketScheme::Plain)),
        );
    }

    #[test]
    fn accepted_link_waits_for_endpoint_facts_without_publishing_ephemeral_ids() {
        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        app.add_plugins(LobbyPlugin::default());
        app.insert_resource(DialConfig {
            config: ClientConfig::default(),
            scheme: WebSocketScheme::Plain,
        });
        app.add_observer(on_endpoint_ready);
        app.add_observer(on_accepted_link);
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 4100));
        let alias = PeerId::Netcode(91);
        let endpoint = app
            .world_mut()
            .spawn((
                WebSocketEndpoint {
                    config: aeronet_websocket::server::ServerConfig::builder()
                        .with_bind_address(address)
                        .with_no_encryption(),
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
            .insert(WebSocketPeerAddr(SocketAddr::from((
                Ipv4Addr::LOCALHOST,
                53000,
            ))));
        assert!(app.world().get::<PendingLobbyPeer>(link).is_none());
        assert!(app.world().get::<RemoteId>(link).is_none());
        app.world_mut()
            .entity_mut(endpoint)
            .insert(WebSocketLocalAddr(address));
        assert!(app.world().get::<PendingLobbyPeer>(link).is_some());
        assert!(app.world().get::<Connected>(link).is_none());
        assert!(app.world().get::<RemoteId>(link).is_none());
        assert!(app.world().get::<LocalId>(link).is_none());
        assert_eq!(app.world().get::<LocalId>(endpoint), Some(&LocalId(alias)));
        assert_eq!(app.world().resource::<Lobby>().local(), Some(alias));
        let changed = SocketAddr::from((Ipv4Addr::LOCALHOST, 4102));
        app.world_mut()
            .entity_mut(endpoint)
            .insert(WebSocketLocalAddr(changed));
        assert_eq!(app.world().resource::<Lobby>().local(), Some(alias));
        assert_eq!(app.world().get::<LocalId>(endpoint), Some(&LocalId(alias)));
        assert!(app.world().get::<PendingLobbyPeer>(link).is_some());
        assert!(app.world().get::<RemoteId>(link).is_none());
    }
}
