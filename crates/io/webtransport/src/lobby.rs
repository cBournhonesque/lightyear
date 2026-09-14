//! Glue between [`lightyear_p2p::Lobby`] and WebTransport links.
//!
//! The lobby discovers peers and emits [`DialPeer`](lightyear_p2p::DialPeer); this module turns that
//! into an Aeronet WebTransport client session, and annotates the sessions peers open towards us so
//! the lobby can see them.
//!
//! # What a peer needs to be dialed
//!
//! More than for WebSocket, because WebTransport authenticates the endpoint with a certificate and a
//! client must know the hash of that certificate in advance. The hash is not derivable from the
//! address, so it travels in the announce as this endpoint's dial context — see
//! [`Lobby::set_dial_context`](lightyear_p2p::Lobby::set_dial_context). Each peer publishes its own, taken from its TLS identity, and relays
//! the ones it learned, so a peer introduced by a third party can still be dialed.
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
//!
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use lightyear_aeronet::endpoint::EndpointAeronetPlugin;
use lightyear_connection::client::{Client, Connect, Connected, Disconnected};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::prelude::{Link, LinkOf, LinkStart, Linked, Unlinked};
use lightyear_p2p::{DialContext, DialPeer, Lobby};
use tracing::{debug, warn};

use aeronet_io::connection::{LocalAddr, PeerAddr};

use crate::client::WebTransportClientIo;
use crate::endpoint::{WebTransportEndpoint, WebTransportEndpointPlugin};

/// Dials WebTransport Links for a [`Lobby`] through a [`WebTransportEndpoint`].
///
/// No configuration: the endpoint's own identity supplies what peers need to dial us, and they
/// supply what we need to dial them.
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
        app.init_resource::<PublishedDigest>();
        app.add_systems(PostUpdate, publish_digest);
        app.add_observer(on_endpoint_added);
        app.add_observer(on_dial);
        app.add_observer(on_accepted_link);
        app.add_observer(on_link_linked);
        app.add_observer(on_accepted_identity);
    }
}

/// The digest we last published, so the endpoint's certificate is hashed once rather than per frame.
#[derive(Resource, Default)]
struct PublishedDigest(Option<String>);

/// Lowercase hex, which is the encoding [`WebTransportClientIo::certificate_digest`] parses.
///
/// Written out rather than pulled from a crate: it is one loop, and it has to match the client's
/// parser exactly.
fn to_hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The hash a peer must expect in order to accept a connection to this endpoint.
fn digest_of(endpoint: &WebTransportEndpoint) -> String {
    let chain = endpoint.certificate.certificate_chain();
    let Some(certificate) = chain.as_slice().first() else {
        return String::new();
    };
    to_hex(certificate.hash().as_ref())
}

/// Tells the lobby how to dial us, so every announce we send carries our certificate hash.
///
/// Until this has run, an announce we send is only half useful: a peer can accept our dial but not
/// start one back.
fn publish_digest(
    mut published: ResMut<PublishedDigest>,
    endpoints: Query<&WebTransportEndpoint>,
    mut lobby: ResMut<Lobby>,
) {
    if published.0.is_some() {
        return;
    }
    let Ok(endpoint) = endpoints.single() else {
        return;
    };
    let digest = digest_of(endpoint);
    published.0 = Some(digest.clone());
    debug!(%digest, "publishing this endpoint's certificate hash so peers can dial us");
    lobby.set_dial_context(DialContext::from(digest));
}

/// Opens the endpoint's socket as soon as the endpoint exists.
///
/// A transport opens on [`LinkStart`], which for a server comes from `Start`. A P2P peer has no
/// server to start, so the glue asks for it directly; without this the endpoint never listens and
/// every dial connects to nothing.
fn on_endpoint_added(trigger: On<Add, WebTransportEndpoint>, mut commands: Commands) {
    commands.trigger(LinkStart {
        entity: trigger.entity,
    });
}

/// The endpoint's bound address, which is this peer's identity.
fn local_id(
    endpoint: Entity,
    addresses: &Query<&LocalAddr, With<WebTransportEndpoint>>,
) -> Option<PeerId> {
    addresses
        .get(endpoint)
        .ok()
        .map(|local| PeerId::Raw(local.0))
}

/// Opens a client session to the peer the lobby asked for.
///
/// Live and pending Links retain their identity reservation. A retry replaces only obsolete
/// WebTransport-owned Links for this peer, keeping the endpoint and its certificate alive.
fn on_dial(
    trigger: On<DialPeer>,
    endpoints: Query<Entity, With<WebTransportEndpoint>>,
    addresses: Query<&LocalAddr, With<WebTransportEndpoint>>,
    existing: Query<
        (
            Entity,
            &RemoteId,
            Has<Disconnected>,
            Has<Unlinked>,
            Has<WebTransportClientIo>,
            Option<&LinkOf>,
        ),
        (With<P2P>, With<Client>),
    >,
    mut commands: Commands,
) {
    let peer = trigger.peer;
    let PeerId::Raw(address) = peer else {
        warn!(
            ?peer,
            "a WebTransport peer must be identified by PeerId::Raw(address); this peer cannot be dialed"
        );
        return;
    };
    let Ok(endpoint) = endpoints.single() else {
        warn!(
            ?peer,
            "no WebTransportEndpoint exists, so no Link can be dialed; spawn a WebTransportEndpoint"
        );
        return;
    };
    let Some(local_id) = local_id(endpoint, &addresses) else {
        warn!(?peer, "the WebTransportEndpoint has no bound address yet");
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

    // `WebTransportClientIo` parses the digest as hex, so the peer's context is passed through as
    // text. A peer that published none cannot be authenticated, and the client is told to expect
    // nothing rather than being handed a malformed hash.
    let certificate_digest = match core::str::from_utf8(&trigger.context) {
        Ok(digest) if !digest.is_empty() => digest.to_string(),
        Ok(_) => {
            warn!(
                ?peer,
                "the peer published no certificate hash, so the connection cannot be authenticated"
            );
            String::new()
        }
        Err(_) => {
            warn!(
                ?peer,
                "the peer's dial context is not text, so its certificate hash is unusable"
            );
            String::new()
        }
    };

    debug!(?peer, %address, %certificate_digest, "lobby dialed a WebTransport peer");
    let link = commands
        .spawn((
            P2P::default(),
            Client,
            LocalId(local_id),
            RemoteId(peer),
            PeerAddr(address),
            Link::default(),
            WebTransportClientIo {
                certificate_digest,
                // Derived from `PeerAddr` as `https://<addr>`.
                target: None,
            },
        ))
        .id();
    commands.trigger(Connect { entity: link });
}

/// Retires obsolete transport-owned Links when an inbound replacement settles its identity.
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

/// Annotates the session a peer opened towards our endpoint.
///
/// The `RemoteId` is provisional: it is the address the session was observed from, which for a
/// stream transport is the peer's ephemeral port rather than its identity. The announce the peer
/// sends over this session carries the real one, and the lobby re-keys the Link to it.
///
/// Only links owned by a [`WebTransportEndpoint`] are touched, so a server's sessions are left alone.
fn on_accepted_link(
    trigger: On<Add, PeerAddr>,
    links: Query<(&LinkOf, &PeerAddr)>,
    addresses: Query<&LocalAddr, With<WebTransportEndpoint>>,
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
    trigger: On<Add, Linked>,
    query: Query<(), (With<P2P>, With<Client>, Without<Connected>)>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(Connected);
    }
}
