//! Glue between [`lightyear_p2p::Lobby`] and WebSocket links.
//!
//! The lobby discovers peers and emits [`DialPeer`](lightyear_p2p::DialPeer); this module turns that
//! into an Aeronet WebSocket client session, and annotates the sessions peers open towards us so the
//! lobby can see them.
//!
//! # What a peer is
//!
//! A [`PeerId::Raw`](lightyear_core::id::PeerId::Raw) holding the peer's socket address, as for UDP:
//! a WebSocket endpoint is one address many peers connect to, and the address is all a client needs
//! to reach it. No dial context is needed, and [`DialPeer::context`] is ignored — WebSocket
//! authenticates with nothing, and its target is the address alone.
//!
//! # Why the identity has to be announced
//!
//! A datagram transport identifies a peer by the address its packets come from. A stream transport
//! cannot: dialing happens from an ephemeral local port, so the endpoint that accepted the session
//! sees `127.0.0.1:52795` rather than the endpoint the peer listens on, and has no way to know who
//! dialed it.
//!
//! The announce fixes this by naming its sender. Accepted Links opt into one address-based identity
//! correction with [`ProvisionalPeerId`](lightyear_p2p::ProvisionalPeerId); dialed Links keep their
//! configured identity. The lobby updates both the Link and the peer lookup, and rejects occupied
//! identities. This is collision prevention, not authentication or proof of endpoint ownership.
//!
//! # Both directions of a session
//!
//! Dialing and being dialed produce different entities, and both are annotated here so a session
//! counts them the same. The lobby's dial tie-break decides which side dials, so a conforming pair
//! only ever produces one.
//!
//! [`DialPeer::context`]: lightyear_p2p::DialPeer::context

use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_aeronet::endpoint::EndpointAeronetPlugin;
use lightyear_connection::client::{Client, Connect, Connected, Disconnected};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::prelude::{Link, LinkOf, LinkStart, Linked, Unlinked};
use lightyear_p2p::DialPeer;
use tracing::{debug, warn};

use aeronet_io::connection::{LocalAddr, PeerAddr};
use aeronet_websocket::client::ClientConfig;

use crate::client::{WebSocketClientIo, WebSocketScheme, WebSocketTarget};
use crate::endpoint::{WebSocketEndpoint, WebSocketEndpointPlugin};

/// Dials WebSocket Links for a [`Lobby`](lightyear_p2p::Lobby) through a [`WebSocketEndpoint`].
///
/// Carries the client configuration because only the application knows what certificate policy its
/// peers accept; the peer addresses themselves come from the lobby.
pub struct WebSocketLobbyPlugin {
    config: ClientConfig,
    scheme: WebSocketScheme,
}

impl WebSocketLobbyPlugin {
    /// Dials peer endpoints using `config`, with `scheme` choosing `ws` or `wss`.
    pub fn new(config: ClientConfig, scheme: WebSocketScheme) -> Self {
        Self { config, scheme }
    }
}

impl Default for WebSocketLobbyPlugin {
    /// Plain `ws` with the default certificate policy.
    fn default() -> Self {
        Self::new(ClientConfig::default(), WebSocketScheme::Plain)
    }
}

/// The client configuration every dialed Link is built with.
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
        app.add_observer(on_dial);
        app.add_observer(on_accepted_link);
        app.add_observer(on_link_linked);
        app.add_observer(on_accepted_identity);
    }
}

/// Opens the endpoint's socket as soon as the endpoint exists.
///
/// A transport opens on [`LinkStart`], which for a server comes from `Start`. A P2P peer has no
/// server to start, so the glue asks for it directly; without this the endpoint never listens and
/// every dial connects to nothing.
fn on_endpoint_added(trigger: On<Add<WebSocketEndpoint>>, mut commands: Commands) {
    commands.trigger(LinkStart {
        entity: trigger.entity,
    });
}

/// The endpoint's bound address, which is this peer's identity.
fn local_id(
    endpoint: Entity,
    addresses: &Query<&LocalAddr, With<WebSocketEndpoint>>,
) -> Option<PeerId> {
    addresses
        .get(endpoint)
        .ok()
        .map(|local| PeerId::Raw(local.0))
}

/// Opens a client session to the peer the lobby asked for.
///
/// Live and pending Links retain their identity reservation. A retry replaces only obsolete
/// WebSocket-owned Links for this peer, including accepted Links, without closing the endpoint.
fn on_dial(
    trigger: On<DialPeer>,
    endpoints: Query<Entity, With<WebSocketEndpoint>>,
    addresses: Query<&LocalAddr, With<WebSocketEndpoint>>,
    existing: Query<
        (
            Entity,
            &RemoteId,
            Has<Disconnected>,
            Has<Unlinked>,
            Has<WebSocketClientIo>,
            Option<&LinkOf>,
        ),
        (With<P2P>, With<Client>),
    >,
    config: Res<DialConfig>,
    mut commands: Commands,
) {
    let peer = trigger.peer;
    let PeerId::Raw(address) = peer else {
        warn!(
            ?peer,
            "a WebSocket peer must be identified by PeerId::Raw(address); this peer cannot be dialed"
        );
        return;
    };
    let Ok(endpoint) = endpoints.single() else {
        warn!(
            ?peer,
            "no WebSocketEndpoint exists, so no Link can be dialed; spawn a WebSocketEndpoint"
        );
        return;
    };
    let Some(local_id) = local_id(endpoint, &addresses) else {
        warn!(?peer, "the WebSocketEndpoint has no bound address yet");
        return;
    };
    // Match the lobby's identity reservation: a pending Link owns its id, but a disconnected
    // or unlinked entity must not suppress application-controlled recovery.
    if existing
        .iter()
        .any(|(_, remote, disconnected, unlinked, _, _)| {
            remote.0 == peer && !disconnected && !unlinked
        })
    {
        debug!(
            ?peer,
            "a Link to this peer already exists; not dialing again"
        );
        return;
    }
    for (entity, remote, disconnected, unlinked, dialed, link_of) in &existing {
        if remote.0 == peer
            && (disconnected || unlinked)
            && (dialed || link_of.is_some_and(|link| link.endpoint == endpoint))
        {
            // LinkOf removal preserves the listening endpoint; AeronetLink's linked-spawn
            // relationship disposes of any remaining session owned by this obsolete Link.
            commands.entity(entity).despawn();
        }
    }

    debug!(?peer, %address, "lobby dialed a WebSocket peer");
    let link = commands
        .spawn((
            P2P::default(),
            Client,
            LocalId(local_id),
            RemoteId(peer),
            PeerAddr(address),
            Link::default(),
            WebSocketClientIo {
                config: config.config.clone(),
                target: WebSocketTarget::Addr(config.scheme),
            },
        ))
        .id();
    commands.trigger(Connect { entity: link });
}

/// Retires obsolete transport-owned Links when an inbound replacement settles its identity.
fn on_accepted_identity(
    trigger: On<Insert<(RemoteId, Connected)>>,
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

/// Annotates the session a peer opened towards our endpoint.
///
/// The `RemoteId` is provisional: it is the address the session was observed from, which for a
/// stream transport is the peer's ephemeral port rather than its identity. The announce the peer
/// sends over this session carries the real one, and the lobby re-keys the Link to it.
///
/// Only links owned by a [`WebSocketEndpoint`] are touched, so a server's sessions are left alone.
fn on_accepted_link(
    trigger: On<Add<PeerAddr>>,
    links: Query<(&LinkOf, &PeerAddr)>,
    addresses: Query<&LocalAddr, With<WebSocketEndpoint>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok((link_of, peer_addr)) = links.get(entity) else {
        // Not an endpoint child: a dialed client session, which `on_dial` already annotated.
        return;
    };
    let Some(local_id) = local_id(link_of.endpoint, &addresses) else {
        return;
    };

    debug!(remote = %peer_addr.0, "lobby endpoint accepted a peer");
    commands.entity(entity).insert((
        P2P::default(),
        Client,
        LocalId(local_id),
        RemoteId(PeerId::Raw(peer_addr.0)),
        lightyear_p2p::ProvisionalPeerId,
    ));
}

/// Makes a `P2P` link count as connected once its session is up.
///
/// The connection layer does not do this for Aeronet-backed transports: only `RawClient` and Steam
/// promote [`Linked`] to [`Connected`], and every `P2P` session reads `Connected`.
fn on_link_linked(
    trigger: On<Add<Linked>>,
    query: Query<(), (With<P2P>, With<Client>, Without<Connected>)>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(Connected);
    }
}
