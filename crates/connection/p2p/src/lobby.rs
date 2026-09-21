//! Lobby-based peer discovery for P2P sessions.
//!
//! A [`Lobby`] answers one question: **which peers should I have a Link to, and which of them
//! belong to the same gathering as me?**; it allows discovery of other P2P links.
//!
//! The lobby is **optional**. An application that already knows its peers declares its Links
//! itself and never installs [`LobbyPlugin`]. The lobby adds discovery and nothing else; it never
//! appears in the session's API.
//!
//! # Discovery
//!
//! Peers exchange [`LobbyAnnounce`]s on the reliable [`P2PChannel`](crate::session::P2PChannel)
//! the session handshake uses. Default dynamic Links first exchange a bounded own-only announce
//! before admission; no provisional peer ID is published. Connected announces are full snapshots
//! of everything the sender knows. The lobby unions them and emits [`DialPeer`] to establish a
//! [`Link`] to peers it is not connected to yet.
//!
//! # Membership
//!
//! Every announce carries the sender's [`LobbyId`], or `None` for a lobby with no id. A peer whose
//! id *disagrees* is recorded as [`PeerState::Foreign`]: it contributes nothing to the lobby's
//! peer set and is never advertised to the rest of the lobby. An absent id on either side is not a
//! disagreement, so a peer that has not settled on an id yet is accepted.
//!
//! The lobby deliberately does **nothing else** about a foreign peer. It does not unlink it, not
//! despawn it, and does not touch its components: whether a connected peer takes part in a session
//! is the application's decision, expressed by the [`P2P`] component.
//!
use alloc::collections::BTreeMap;
use alloc::collections::btree_map::Entry;
use alloc::string::String;
use alloc::vec::Vec;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bytes::Bytes;
#[cfg(test)]
use lightyear_connection::admission::NoDefaultConnection;
use lightyear_connection::admission::{admit, is_default_connection, reject};
use lightyear_connection::client::{Connected, Disconnect, Disconnected};
use lightyear_connection::direction::NetworkDirection;
#[cfg(test)]
use lightyear_connection::network_topology::NetworkingMetadata;
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::prelude::{LinkOf, Unlinked};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{
    AppMessageExt, MessageReceiver, MessageSender, PreadmissionMessage,
};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use tracing::{debug, trace, warn};

use crate::session::{P2PChannel, P2PProtocolPlugin};

/// Identity of a lobby.
///
/// A **membership token, not an address**: nothing ever resolves or dials a `LobbyId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LobbyId([u8; 32]);

impl LobbyId {
    /// Builds an id from raw bytes. Generate them randomly when opening a lobby.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// How a lobby decides which gathering it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LobbyIdPolicy {
    /// No id of our own: adopt the id of the first peer whose announce is accepted, then require
    /// agreement with it.
    ///
    /// This is the joiner case, where the invite carries only endpoint ids. Until an announce
    /// arrives we announce `None` and accept everyone, so a peer that has not settled is never
    /// mistaken for a peer that disagrees.
    #[default]
    Adopt,
    /// Announce this id and accept only peers that agree.
    ///
    /// This is the peer that opens a lobby (it mints the id) and the pinned-invite case, where a
    /// mismatched invite should fail loudly instead of silently merging two gatherings.
    ///
    /// `Pinned(None)` is a lobby **with no id**: it announces no id and therefore accepts every
    /// peer, which is what an application that does not care about separating gatherings wants.
    Pinned(Option<LobbyId>),
}

/// What the lobby knows about one peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    /// Known from an announce, but no Link to it yet. It is dialed, and it is re-announced so the
    /// rest of the lobby can reach it too.
    Wanted,
    /// Has a Link and announced a lobby that agrees with ours: part of the lobby.
    Member,
    /// Has a Link but announced a lobby that disagrees with ours. Never advertised, never part of
    /// the lobby's peer set. The lobby does nothing further about it.
    Foreign,
}

/// Transport-defined bytes that tell a peer how to dial whoever they describe.
///
/// The lobby treats this as opaque: only the transport's lobby glue knows how to produce it and how
/// to read it. It exists because an identity and an address are different facts — a transport that
/// addresses peers by key (rather than by a socket address that is already the [`PeerId`]) has to
/// publish that key, and the lobby is what carries the publication.
///
/// Reference-counted, so cloning it costs nothing: an announce is cloned once per Link before it is
/// sent, and every entry of [`LobbyAnnounce::known`] holds one. Nothing here is trusted — see
/// [`MAX_DIAL_CONTEXT`].
pub type DialContext = Bytes;

/// Longest [`DialContext`] the lobby will store from an announce.
///
/// The context arrives from an untrusted peer, so it is bounded before being kept. A longer one is
/// discarded rather than truncated: a transport would misread a partial key, and dialing without a
/// key is a clean failure that `Lobby::retry` can recover from.
pub const MAX_DIAL_CONTEXT: usize = 64;

/// Describes the sender's lobby and dialing context, and after admission the peers it knows.
///
/// The first announcement contains only the sender. Connected announcements are full snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyAnnounce {
    /// The sender's lobby, or `None` if it has not settled on one.
    pub lobby: Option<LobbyId>,
    /// Who sent this.
    ///
    /// Before admission this establishes the final remote identity. Once connected it must agree
    /// with the Link's immutable [`RemoteId`]; a different sender is rejected, never renamed.
    pub from: PeerId,
    /// Every peer the sender knows how to dial, **including itself**, with how to dial it.
    ///
    /// The sender is in the list on purpose. Nothing else would carry how to dial it, and without
    /// that a peer whose Link drops could not be dialed again ([`Lobby::retry`]). Having one list
    /// rather than a list plus a separate field also means a receiver needs no special case: the
    /// sender's own entry is learned like any other.
    ///
    /// Per-peer contexts are what make an announce *hop* work: the peer that introduces two others
    /// is the only one holding the key for the introduced peer, so it has to pass it along. A peer
    /// with no context carries an empty one, and a transport that can derive a target from the
    /// [`PeerId`] alone does not need one at all.
    ///
    /// A `SmallVec` rather than a `Vec` because the message is cloned once per Link before being
    /// serialized (`MessageSender::send` takes its message by value), and a lobby is a handful of
    /// peers. Inline, that copy — build and every clone — allocates nothing. The inline capacity
    /// holds a full lobby plus the sender, matching the convention `P2PSession` uses for its lists.
    pub known: SmallVec<[(PeerId, DialContext); 5]>,
}

/// Emitted when the lobby wants a Link to a peer it has not dialed yet.
///
/// The transport glue turns this into a Link. `peer` is the lobby's own vocabulary — a [`PeerId`] —
/// and turning it into whatever the transport actually dials is the glue's job, because the lobby
/// never names a transport.
///
/// # Resolving a peer to a dial target
///
/// An identity and an address are different facts, and only the transport knows how they relate.
/// Three shapes, in increasing order of what the transport must supply:
///
/// 1. **The id is the address.** A transport that identifies a peer by its socket address uses
///    [`PeerId::Raw`], so `peer` already *is* the target and the glue needs nothing else. This is
///    what default socket connections do: they identify the remote as `PeerId::Raw(peer_addr)`.
/// 2. **The address is derivable.** A transport whose addressing is a function of its own
///    configuration computes the target from `peer` plus that configuration — a UDP mesh that
///    derives ports from a base port and the peer's slot, for instance.
/// 3. **The address is supplied out of band.** When neither holds — a peer known only by an
///    unguessable key — the application must be told where that peer is, and the glue keeps that
///    mapping. `lightyear_iroh`'s `IrohLobbyAddrs` is this case: an Iroh endpoint id carries no
///    address, so an invite's addresses are inserted there.
///
/// The lobby cannot help with 2 or 3 and does not try to. What it does guarantee is that it never
/// emits this for a peer that is already linked, already dialed, or known to be in another lobby
/// ([`Lobby::retry`] re-arms a peer, which is the only way it is emitted twice) — so a glue may
/// dial on every event without defending against repeats.
#[derive(Event, Debug, Clone)]
pub struct DialPeer {
    /// The peer to reach, in the lobby's vocabulary. See the type docs for how a transport turns
    /// this into its own dial target.
    pub peer: PeerId,
    /// Transport-defined bytes from the peer's own announce, needed to dial it when the [`PeerId`]
    /// alone is not a target. Empty when the lobby has never heard one.
    pub context: DialContext,
}

/// Everything the lobby knows about one peer.
///
/// The three facts are independent — what we decided about the peer, whether its Link is up, and
/// whether we have already tried to reach it — so each is a field here rather than an entry in a
/// set beside the map. Keeping them on the entry is what makes `forget_disconnected` and
/// `retry_failed` single loops instead of code that has to keep three collections in step.
#[derive(Debug, Clone)]
struct PeerInfo {
    /// Our verdict on the peer's lobby. `Wanted` until an announce settles it.
    state: PeerState,
    /// Whether a Link to this peer is connected right now.
    linked: bool,
    /// Whether a dial was attempted or a member's previous connection needs an explicit retry.
    dialed: bool,
    /// How to dial this peer.
    context: DialContext,
    /// A direct announcement owns this context, including an explicitly empty context.
    direct_context: bool,
    /// Whether we are obliged to dial this peer even if the tie-break says the other side should.
    ///
    /// True for a peer the application named: a bootstrap seed, or one re-armed with
    /// [`Lobby::retry`]. The tie-break assumes the other peer will dial us, which is only safe when
    /// that peer knows we exist — and a peer we were told about by the application may not. Waiting
    /// on a peer that never learned of us stalls forever, so these are dialed regardless of ids.
    dial_explicitly: bool,
}

impl PeerInfo {
    /// A peer we have heard of and done nothing about yet.
    fn known() -> Self {
        Self {
            state: PeerState::Wanted,
            linked: false,
            dialed: false,
            context: DialContext::new(),
            direct_context: false,
            dial_explicitly: false,
        }
    }

    /// Re-arms a failed dial or disconnected member once until the next attempt.
    fn rearm(&mut self) -> bool {
        if self.state == PeerState::Foreign
            || (self.linked && (self.state != PeerState::Wanted || !self.dialed))
            || (!self.dialed && self.dial_explicitly)
        {
            return false;
        }
        self.dialed = false;
        self.dial_explicitly = true;
        true
    }
}

/// A lobby Link whose remote identity has not yet been admitted.
///
/// No public Link IDs or connectivity are published until its bounded own-only announcement is
/// accepted. Transport glue starts this exchange with [`begin_peer_resolution`].
#[derive(Component, Clone, Copy)]
#[component(immutable)]
pub struct PendingLobbyPeer {
    endpoint: Entity,
    local: PeerId,
    expected: Option<PeerId>,
}

impl PendingLobbyPeer {
    /// The dial target whose identity must match the first remote announcement.
    pub fn expected_remote(&self) -> Option<PeerId> {
        self.expected
    }
}

#[derive(Component)]
#[component(immutable)]
struct EndpointLobbyIdentity(PeerId);

#[derive(Component)]
struct RejectedLobbyPeer;

/// Freeze one self-ID for a lobby endpoint, preserving an explicitly requested [`LocalId`].
///
/// `candidate` is the transport's native fallback, not a replacement for a chosen identity.
/// All endpoints participating in this lobby must agree; inconsistent requests close the endpoint.
pub fn resolve_endpoint_identity(
    world: &mut World,
    endpoint: Entity,
    candidate: PeerId,
) -> Option<PeerId> {
    if world.get_entity(endpoint).is_err() || !world.contains_resource::<Lobby>() {
        return None;
    }
    let local = world.get::<LocalId>(endpoint).map_or(candidate, |id| id.0);
    if world
        .get::<EndpointLobbyIdentity>(endpoint)
        .is_some_and(|id| id.0 != local)
        || world
            .resource::<Lobby>()
            .local
            .is_some_and(|id| id != local)
    {
        reject(
            world,
            endpoint,
            String::from("Conflicting lobby endpoint self-ID"),
        );
        return None;
    }
    world.resource_mut::<Lobby>().mark_local(local);
    world
        .entity_mut(endpoint)
        .insert((LocalId(local), EndpointLobbyIdentity(local)));
    Some(local)
}

/// Begin the ordinary reliable lobby announcement exchange before default admission.
///
/// Call only once the endpoint's native identity and dialing context are ready. A dialed peer
/// supplies `expected`; an incoming Link supplies `None`. Link-level requests must agree with
/// the endpoint and dial target. Repeated calls do not resend or reopen an admitted/rejected Link.
pub fn begin_peer_resolution(
    world: &mut World,
    entity: Entity,
    endpoint: Entity,
    expected: Option<PeerId>,
) -> bool {
    if world.get_entity(entity).is_err()
        || !world.contains_resource::<Lobby>()
        || world.get::<Connected>(entity).is_some()
        || world.get::<RejectedLobbyPeer>(entity).is_some()
        || world.get::<Disconnected>(entity).is_some()
        || world.get::<Unlinked>(entity).is_some()
    {
        return false;
    }
    if !is_default_connection(world, entity) || !is_default_connection(world, endpoint) {
        return false;
    }
    let Some(local) = world.get::<EndpointLobbyIdentity>(endpoint).map(|id| id.0) else {
        reject_lobby_peer(world, entity, "Lobby endpoint identity is unresolved");
        return false;
    };
    if world.get::<LocalId>(endpoint).map(|id| id.0) != Some(local)
        || world.get_resource::<Lobby>().and_then(Lobby::local) != Some(local)
        || world
            .get::<LinkOf>(entity)
            .is_some_and(|link| link.endpoint != endpoint)
        || world.get::<LocalId>(entity).is_some_and(|id| id.0 != local)
        || world
            .get::<RemoteId>(entity)
            .is_some_and(|id| Some(id.0) != expected)
        || expected == Some(local)
    {
        reject_lobby_peer(world, entity, "Conflicting lobby Link identity request");
        return false;
    }
    if let Some(pending) = world.get::<PendingLobbyPeer>(entity) {
        if pending.endpoint == endpoint && pending.local == local && pending.expected == expected {
            return true;
        }
        reject_lobby_peer(
            world,
            entity,
            "Cannot change a pending lobby identity request",
        );
        return false;
    }
    let lobby = world.resource::<Lobby>();
    let mut known = SmallVec::new();
    known.push((local, lobby.dial_context.clone()));
    let announce = LobbyAnnounce {
        lobby: lobby.id,
        from: local,
        known,
    };
    // Always queue our own announcement before receiving theirs. Admission may happen in the
    // same frame, but removing the preadmission gate must not discard this reliable response.
    let mut sender = MessageSender::<LobbyAnnounce>::default();
    sender.send::<P2PChannel>(announce);
    world
        .entity_mut(entity)
        .remove::<(LocalId, RemoteId)>()
        .insert((
            PendingLobbyPeer {
                endpoint,
                local,
                expected,
            },
            PreadmissionMessage::new::<LobbyAnnounce>(256),
            sender,
            MessageReceiver::<LobbyAnnounce>::default(),
        ));
    true
}

fn reject_lobby_peer(world: &mut World, entity: Entity, reason: &str) {
    world
        .entity_mut(entity)
        .remove::<(PendingLobbyPeer, PreadmissionMessage)>()
        .insert(RejectedLobbyPeer);
    reject(world, entity, String::from(reason));
}

/// Peer discovery for a P2P application.
///
/// Holds the lobby identity, the peers known to be in it, and which of them currently have a Link.
/// Read it to decide when and with whom to start a session; the lobby itself takes no part in that
/// decision.
#[derive(Resource, Debug)]
pub struct Lobby {
    /// Whether the first accepted announce decides our id.
    adopting: bool,
    /// Our lobby.
    id: Option<LobbyId>,
    /// The local peer's frozen endpoint identity, inherited by its child links.
    local: Option<PeerId>,
    /// Peers we have learned of, and what we know about them.
    ///
    /// Connectivity is maintained by observers on `Connected`, never recomputed: every way a Link
    /// can come up or go down ends in that component being added or removed (see `LobbyPlugin`),
    /// and the lobby has nothing else to read.
    peers: BTreeMap<PeerId, PeerInfo>,
    /// Whether there is something to announce that our peers have not been told.
    dirty: bool,
    /// How our peers should dial us, published in every announce we send.
    dial_context: DialContext,
}

/// Keeps a context from an untrusted peer only if it is a size a transport can read.
fn bounded_context(context: DialContext) -> DialContext {
    if context.len() > MAX_DIAL_CONTEXT {
        warn!(
            len = context.len(),
            max = MAX_DIAL_CONTEXT,
            "discarding an oversized dial context from an announce"
        );
        return DialContext::new();
    }
    context
}

impl Lobby {
    fn new(id_policy: LobbyIdPolicy) -> Self {
        let (id, adopting) = match id_policy {
            LobbyIdPolicy::Pinned(id) => (id, false),
            // Unsettled: the first accepted announce decides.
            LobbyIdPolicy::Adopt => (None, true),
        };
        Self {
            adopting,
            id,
            local: None,
            peers: BTreeMap::new(),
            dirty: true,
            dial_context: DialContext::new(),
        }
    }

    /// Sets the bytes our peers get in every announce so they can dial us.
    ///
    /// The transport's lobby glue owns the encoding; the lobby only carries it. Marks the lobby
    /// dirty, because peers that already have our previous context need the new one.
    pub fn set_dial_context(&mut self, context: DialContext) {
        let context = bounded_context(context);
        if self.dial_context == context {
            return;
        }
        self.dial_context = context;
        self.dirty = true;
    }

    /// The transport-owned context to include alongside our self-ID in an invite.
    pub fn local_dial_context(&self) -> &DialContext {
        &self.dial_context
    }

    /// How to dial `peer`, as last announced. Empty when nobody has said.
    ///
    /// The transport's lobby glue reads this when it turns a [`DialPeer`] into a Link.
    pub fn dial_context(&self, peer: PeerId) -> DialContext {
        self.peers
            .get(&peer)
            .map(|info| info.context.clone())
            .unwrap_or_default()
    }

    /// The lobby's identity, or `None` if it has none (or has not settled on one yet).
    pub fn id(&self) -> Option<LobbyId> {
        self.id
    }

    /// The local peer's id, once a Link has been seen.
    pub fn local(&self) -> Option<PeerId> {
        self.local
    }

    /// Every peer we have learned of, and what we know about it.
    pub fn peers(&self) -> impl Iterator<Item = (PeerId, PeerState)> + '_ {
        self.peers.iter().map(|(peer, info)| (*peer, info.state))
    }

    /// The peers in this lobby, whether or not they are currently connected.
    pub fn members(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.peers
            .iter()
            .filter(|(_, info)| info.state == PeerState::Member)
            .map(|(peer, _)| *peer)
    }

    /// The lobby's members that currently have a Link.
    ///
    /// The usual input to an application's own "everyone is here" check.
    pub fn connected_members(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.members().filter(|peer| self.is_connected(*peer))
    }

    /// Peers we know of but have no Link to yet.
    pub fn waiting(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.peers
            .iter()
            .filter(|(peer, info)| info.state == PeerState::Wanted && !self.is_connected(**peer))
            .map(|(peer, _)| *peer)
    }

    /// Whether we currently have a connected Link to `peer`.
    pub fn is_connected(&self, peer: PeerId) -> bool {
        self.peers.get(&peer).is_some_and(|info| info.linked)
    }

    /// Whether `peer` is part of this lobby.
    pub fn is_member(&self, peer: PeerId) -> bool {
        self.peers
            .get(&peer)
            .is_some_and(|info| info.state == PeerState::Member)
    }

    /// The lobby's members plus ourselves, sorted.
    ///
    /// Sorting is by [`PeerId`]'s derived order, which every peer computes identically, so
    /// [`slot_of`](Self::slot_of) needs no coordination between peers.
    pub fn roster(&self) -> Vec<PeerId> {
        let mut roster: Vec<PeerId> = self.members().collect();
        if let Some(local) = self.local {
            roster.push(local);
        }
        roster.sort_unstable();
        roster.dedup();
        roster
    }

    /// This peer's slot in the roster, or `None` while the roster is empty.
    pub fn local_slot(&self) -> Option<u8> {
        let local = self.local?;
        self.slot_of(local)
    }

    /// `peer`'s slot in the roster.
    pub fn slot_of(&self, peer: PeerId) -> Option<u8> {
        let slot = self.roster().iter().position(|p| *p == peer)?;
        u8::try_from(slot).ok()
    }

    /// Records peers the lobby should dial, before anything is connected.
    ///
    /// This is the invite case: the application read endpoint ids out of a URL (or its own config)
    /// and hands them over. Each becomes [`PeerState::Wanted`] and is dialed by
    /// the lobby's dial emitter, so the application never creates Links itself.
    pub fn add_bootstrap(&mut self, peers: impl IntoIterator<Item = PeerId>) {
        self.add_bootstrap_with_context(peers.into_iter().map(|peer| (peer, DialContext::new())));
    }

    /// [`add_bootstrap`](Self::add_bootstrap) for a transport whose targets need more than the
    /// [`PeerId`].
    ///
    /// A seed is reached before any announce can arrive, so a transport that cannot derive a target
    /// from the id alone — one that needs a certificate digest, say — has no other way to learn it.
    pub fn add_bootstrap_with_context(
        &mut self,
        peers: impl IntoIterator<Item = (PeerId, DialContext)>,
    ) {
        for (peer, context) in peers {
            self.learn(peer, context);
            if let Some(info) = self.peers.get_mut(&peer) {
                info.dial_explicitly = true;
            }
        }
    }

    /// Asks for `peer` to be dialed again.
    ///
    /// A dial is attempted once. This re-arms a failed attempt or a disconnected
    /// [`PeerState::Member`], retaining its membership and dial context. An unlinked peer is dialed
    /// on the next dispatch regardless of the automatic dial tie-break.
    ///
    /// Returns `true` only when newly re-armed. Unknown peers, [`PeerState::Foreign`] peers,
    /// connected members, and peers already re-armed before dispatch return `false`.
    /// Retrying is the application's decision and its pacing: the lobby has no clock.
    ///
    /// A peer that is [`PeerState::Wanted`] is retryable even while a Link exists, because a Link is
    /// not evidence that anyone is listening: a connectionless transport reports one as soon as it
    /// is dialed. Such a peer re-announces after an attempted dial; an inbound-only Wanted Link
    /// has no failed attempt to retry.
    ///
    /// Re-arming also marks the lobby dirty. On a transport where dialing carries an announce, the
    /// announce that went with the failed dial is as likely to be what was lost as the dial itself,
    /// so a retry has to speak again rather than only repeat a transport call.
    pub fn retry(&mut self, peer: PeerId) -> bool {
        let Some(info) = self.peers.get_mut(&peer) else {
            return false;
        };
        let rearmed = info.rearm();
        self.dirty |= rearmed;
        rearmed
    }

    /// Re-arms failed Wanted peers and disconnected members, as [`retry`](Self::retry) does.
    ///
    /// Returns how many were newly re-armed; repeated calls before dispatch count them only once.
    /// Members retain their membership and dial context. Foreign peers and connected members are
    /// excluded, while a dialed Wanted peer may re-announce over its existing connectionless Link.
    pub fn retry_failed(&mut self) -> usize {
        let mut rearmed = 0;
        for info in self.peers.values_mut() {
            if info.rearm() {
                rearmed += 1;
            }
        }
        if rearmed > 0 {
            // See `retry`: a failed dial must be able to re-announce, not just re-dial.
            self.dirty = true;
        }
        rearmed
    }

    /// Forgets every peer with no Link, so they stop being advertised and counted.
    ///
    /// Without this the peer set only ever grows: a departed peer stays a [`PeerState::Member`] and
    /// is re-announced to everyone for the rest of the process's life. The lobby cannot decide this
    /// for itself — it has no clock, and a peer that is merely slow looks exactly like one that has
    /// left — so the application calls it when it knows, such as between rounds.
    ///
    /// Returns how many peers were forgotten. A forgotten peer is learned again if it comes back,
    /// because it either dials us or another peer advertises it.
    pub fn forget_disconnected(&mut self) -> usize {
        let before = self.peers.len();
        // Dropping the entry drops its `dialed` flag too, so nothing is left behind as a dial
        // target.
        self.peers.retain(|_, info| info.linked);
        if self.peers.len() != before {
            self.dirty = true;
        }
        before - self.peers.len()
    }

    /// Records that we have asked the transport to dial `peer`, so we do not ask twice.
    fn mark_dialed(&mut self, peer: PeerId) {
        if let Some(info) = self.peers.get_mut(&peer) {
            info.dialed = true;
        }
    }

    /// Whether `peer` announced a lobby that is not ours.
    fn is_elsewhere(&self, peer: PeerId) -> bool {
        self.peers
            .get(&peer)
            .is_some_and(|info| info.state == PeerState::Foreign)
    }

    /// Records our own peer id, learned from a Link's [`LocalId`].
    ///
    /// Our id can also reach us inside a remote announce — a peer that knows us lists us — before
    /// any Link has told us which id is ours. Settling it here drops any such trace of ourselves,
    /// so we are never advertised, dialed, or counted as a member.
    fn mark_local(&mut self, local: PeerId) -> bool {
        if let Some(current) = self.local {
            return current == local;
        }
        self.local = Some(local);
        self.dirty = true;
        self.peers.remove(&local);
        true
    }

    /// Records that the Link to `peer` is up.
    fn mark_linked(&mut self, peer: PeerId) {
        if self.local == Some(peer) {
            return;
        }
        let mut changed = false;
        match self.peers.entry(peer) {
            Entry::Occupied(mut slot) => {
                let info = slot.get_mut();
                if !info.linked {
                    info.linked = true;
                    changed = true;
                }
            }
            Entry::Vacant(slot) => {
                // A connected Link is a peer even before it announces.
                slot.insert(PeerInfo {
                    linked: true,
                    ..PeerInfo::known()
                });
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    /// Records that the Link to `peer` is gone.
    ///
    /// Deliberately does **not** set `dirty`: the announce carries membership and the peers we
    /// know, neither of which changes when a Link goes away (a departed peer stays a member).
    fn mark_unlinked(&mut self, peer: PeerId) {
        if let Some(info) = self.peers.get_mut(&peer) {
            if info.linked && info.state == PeerState::Member {
                // An inbound connection also consumes the initial attempt: losing it must not
                // silently dial again just because this side is lower in the tie-break.
                info.dialed = true;
            }
            info.linked = false;
        }
    }

    /// Records a peer we have heard of but not verified, and how to dial it.
    ///
    /// A context we already hold is never overwritten by an empty one: the peer that introduced two
    /// others may have a key where a later announce does not, and losing it would strand a dial.
    fn learn(&mut self, peer: PeerId, context: DialContext) {
        if self.local == Some(peer) {
            return;
        }
        match self.peers.entry(peer) {
            Entry::Occupied(mut slot) => {
                let info = slot.get_mut();
                if !info.direct_context && info.context.is_empty() && !context.is_empty() {
                    info.context = context;
                }
            }
            // Only insert if we do not already track the peer: a third party's announce may race
            // ahead of our own membership decision, and `Member`/`Foreign` must not be downgraded
            // back to `Wanted`.
            Entry::Vacant(slot) => {
                slot.insert(PeerInfo {
                    context,
                    ..PeerInfo::known()
                });
                self.dirty = true;
            }
        }
    }

    /// Whether a lobby id from a peer agrees with ours.
    ///
    /// An absent id on either side is not a disagreement: there is nothing to compare, and a peer
    /// that has not settled yet must still be admitted.
    fn agrees(&self, other: Option<LobbyId>) -> bool {
        match (self.id, other) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => true,
        }
    }

    /// Records `peer`'s membership state.
    fn set_state(&mut self, peer: PeerId, state: PeerState) {
        let mut changed = false;
        match self.peers.entry(peer) {
            Entry::Occupied(mut slot) => {
                let info = slot.get_mut();
                if info.state != state {
                    info.state = state;
                    changed = true;
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(PeerInfo {
                    state,
                    ..PeerInfo::known()
                });
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    /// Records an announce received on the Link identified by `from`.
    fn accept_announce(&mut self, from: PeerId, announce: LobbyAnnounce) {
        if self.local == Some(from) {
            return;
        }

        if self.adopting {
            // Settle on the first accepted announce, even if it names no lobby at all.
            self.id = announce.lobby;
            self.adopting = false;
            self.dirty = true;
        }

        trace!(
            ?from,
            lobby = ?announce.lobby,
            known = announce.known.len(),
            "received a lobby announce"
        );

        if !self.agrees(announce.lobby) {
            trace!(?from, "peer announced a different lobby");
            self.set_state(from, PeerState::Foreign);
            return;
        }

        if self.peers.get(&from).map(|info| info.state) != Some(PeerState::Member) {
            debug!(?from, "peer joined the lobby");
        }
        self.set_state(from, PeerState::Member);
        for (peer, context) in announce.known {
            if peer == from {
                let context = bounded_context(context);
                let info = self
                    .peers
                    .get_mut(&from)
                    .expect("sender membership was recorded");
                if info.context != context {
                    info.context = context;
                    self.dirty = true;
                }
                info.direct_context = true;
            } else {
                self.learn(peer, bounded_context(context));
            }
        }
    }

    /// Peers to dial: known, never dialed, not connected, and not known to be elsewhere.
    ///
    /// Our own id cannot appear here: [`learn`] refuses it, and [`mark_local`] purges it if it was
    /// learned before we knew which id was ours.
    ///
    /// # The tie-break
    ///
    /// Only the lower of two ids dials the other; see [`should_dial`](Self::should_dial).
    fn to_dial(&self) -> Vec<PeerId> {
        self.peers
            .iter()
            .filter(|(peer, info)| {
                info.state != PeerState::Foreign
                    && !info.linked
                    && !info.dialed
                    && self.should_dial(**peer, info)
            })
            .map(|(peer, _)| *peer)
            .collect()
    }

    /// Whether we are the side that dials `peer`, rather than the side that waits to be dialed.
    ///
    /// Two peers that discover each other at the same moment would otherwise both dial, and a
    /// transport that cannot absorb that — one where dialing and accepting produce different
    /// entities, as WebTransport does — ends up with two Links to one peer, which
    /// [`P2PStart`](crate::P2PStart) rejects outright as a duplicate identity. Ordering the dial by
    /// id removes the duplicate by construction, and every peer computes the same order from ids it
    /// already has, so it needs no coordination.
    ///
    /// Exemptions:
    /// - A peer the application asked for ([`add_bootstrap`](Self::add_bootstrap), or a
    ///   [`retry`](Self::retry)) is always dialed. An invitee has to dial its host whatever the ids
    ///   say, and the host may not know the invitee exists.
    /// - Before our own id is known there is nothing to compare, so we dial. That is the safe
    ///   direction: a redundant dial is recoverable, silence is not.
    ///
    /// A peer we skip stays [`PeerState::Wanted`] but was never dialed, so re-arming it is what
    /// [`retry`](Self::retry) does. That is the recovery for the case this cannot cover: we are the
    /// higher id, so we wait, but the peer never learned of us and so never dials.
    fn should_dial(&self, peer: PeerId, info: &PeerInfo) -> bool {
        if info.dial_explicitly {
            return true;
        }
        match self.local {
            Some(local) => local < peer,
            None => true,
        }
    }

    /// Peers to announce: ourselves, everyone in the lobby, and anyone still unverified.
    ///
    /// Ourselves so that a peer that loses its Link to us can dial us again. A
    /// [`PeerState::Foreign`] peer is deliberately absent: the lobby does not spread membership of
    /// gatherings that are not its own.
    fn known(&self) -> SmallVec<[(PeerId, DialContext); 5]> {
        let mut known: SmallVec<[(PeerId, DialContext); 5]> = self
            .peers
            .iter()
            .filter(|(_, info)| matches!(info.state, PeerState::Member | PeerState::Wanted))
            .map(|(peer, info)| (*peer, info.context.clone()))
            .collect();
        if let Some(local) = self.local {
            known.push((local, self.dial_context.clone()));
        }
        known
    }

    /// The announce to send to our peers, or `None` while we do not know our own id.
    ///
    /// An announce names its sender, so there is nothing truthful to send before a Link has told us
    /// our own id — and nothing to send it over either.
    fn announce(&self) -> Option<LobbyAnnounce> {
        Some(LobbyAnnounce {
            lobby: self.id,
            from: self.local?,
            known: self.known(),
        })
    }
}

/// Installs lobby-based peer discovery.
///
/// Add this alongside [`P2PSessionPlugin`](crate::P2PSessionPlugin) when the set of peers is not
/// known up front. Peer Links are those carrying [`P2P`], which is what the transport's glue
/// marks its sessions with and what the application marks its own Links with; the glue also handles
/// [`DialPeer`].
pub struct LobbyPlugin {
    id_policy: LobbyIdPolicy,
}

impl LobbyPlugin {
    /// `id_policy` decides which lobby this peer belongs to: [`LobbyIdPolicy::Pinned`] for the peer
    /// that opens one (or for a peer whose invite named it), [`LobbyIdPolicy::Adopt`] for a peer
    /// that was invited by id alone.
    pub fn new(id_policy: LobbyIdPolicy) -> Self {
        Self { id_policy }
    }
}

impl Default for LobbyPlugin {
    fn default() -> Self {
        Self::new(LobbyIdPolicy::default())
    }
}

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        // The announces share the session's channel rather than reserving one of their own.
        if !app.is_plugin_added::<P2PProtocolPlugin>() {
            app.add_plugins(P2PProtocolPlugin);
        }
        app.register_message::<LobbyAnnounce>()
            .add_direction(NetworkDirection::Bidirectional);

        app.insert_resource(Lobby::new(self.id_policy));

        app.add_observer(on_link_connected);
        app.add_observer(on_link_disconnected);
        app.add_observer(on_pending_disconnect);
        app.add_observer(on_pending_disconnected);
        app.add_observer(on_pending_unlinked);
        app.add_systems(
            PostUpdate,
            // Flush our own pending announcement before admission exposes any other messages.
            // No timer or acknowledgement is needed: the existing ordered channel owns delivery.
            (drain_announcements, announce_changes, dial_new_peers)
                .chain()
                .after(MessageSystems::Send),
        );
    }
}

/// Drain ordinary messages first, then admit synchronously so two claims cannot reserve one ID.
fn drain_announcements(world: &mut World) {
    let mut query =
        world.query_filtered::<(Entity, &mut MessageReceiver<LobbyAnnounce>), With<P2P>>();
    let mut incoming = Vec::new();
    for (entity, mut receiver) in query.iter_mut(world) {
        incoming.extend(receiver.receive().map(|message| (entity, message)));
    }
    for (entity, message) in incoming {
        accept_link_announce(world, entity, message);
    }
}

fn accept_link_announce(world: &mut World, entity: Entity, message: LobbyAnnounce) {
    if world.get::<RejectedLobbyPeer>(entity).is_some()
        || world.get::<Disconnected>(entity).is_some()
        || world.get::<Unlinked>(entity).is_some()
    {
        return;
    }
    let from = message.from;
    if let Some(pending) = world.get::<PendingLobbyPeer>(entity).copied() {
        if pending.expected.is_some_and(|expected| expected != from)
            || from == pending.local
            || world.get::<LocalId>(pending.endpoint).map(|id| id.0) != Some(pending.local)
            || world.resource::<Lobby>().local != Some(pending.local)
            || message.known.len() != 1
            || message.known[0].0 != from
            || message.known[0].1.len() > MAX_DIAL_CONTEXT
        {
            reject_lobby_peer(world, entity, "Invalid preadmission lobby announcement");
            return;
        }
        if !admit(world, entity, LocalId(pending.local), RemoteId(from), ()) {
            world
                .entity_mut(entity)
                .remove::<(PendingLobbyPeer, PreadmissionMessage)>()
                .insert(RejectedLobbyPeer);
            return;
        }
        world
            .entity_mut(entity)
            .remove::<(PendingLobbyPeer, PreadmissionMessage)>();
    } else if world.get::<Connected>(entity).is_none() {
        return;
    } else if world.get::<RemoteId>(entity).map(|id| id.0) != Some(from) {
        reject_lobby_peer(
            world,
            entity,
            "Cannot change an admitted lobby peer identity",
        );
        return;
    }
    world.resource_mut::<Lobby>().accept_announce(from, message);
}

fn clear_pending(world: &mut World, entity: Entity) {
    if world.get::<PendingLobbyPeer>(entity).is_some() {
        world
            .entity_mut(entity)
            .remove::<(PendingLobbyPeer, PreadmissionMessage)>()
            .insert(RejectedLobbyPeer);
    }
}

fn on_pending_disconnect(trigger: On<Disconnect>, mut commands: Commands) {
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| clear_pending(world, entity));
}

fn on_pending_disconnected(trigger: On<Add, Disconnected>, mut commands: Commands) {
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| clear_pending(world, entity));
}

fn on_pending_unlinked(trigger: On<Add, Unlinked>, mut commands: Commands) {
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| clear_pending(world, entity));
}

/// Records a Link coming up: the peer is linked, and we now know our own id.
fn on_link_connected(
    trigger: On<Add, Connected>,
    mut commands: Commands,
    links: Query<(&RemoteId, &LocalId), With<P2P>>,
    mut lobby: ResMut<Lobby>,
) {
    let Ok((remote_id, local_id)) = links.get(trigger.entity) else {
        return;
    };
    if !lobby.mark_local(local_id.0) {
        let entity = trigger.entity;
        commands.queue(move |world: &mut World| {
            reject_lobby_peer(world, entity, "Connected Link conflicts with lobby self-ID");
        });
        return;
    }
    lobby.mark_linked(remote_id.0);
}

/// Records the last live Link to a peer going away, ignoring obsolete replacement entities.
fn on_link_disconnected(
    trigger: On<Remove, Connected>,
    links: Query<&RemoteId, With<P2P>>,
    connected: Query<
        (Entity, &RemoteId),
        (
            With<P2P>,
            With<Connected>,
            Without<Disconnected>,
            Without<Unlinked>,
        ),
    >,
    mut lobby: ResMut<Lobby>,
) {
    let Ok(remote_id) = links.get(trigger.entity) else {
        return;
    };
    if connected
        .iter()
        .any(|(entity, id)| entity != trigger.entity && id.0 == remote_id.0)
    {
        return;
    }
    lobby.mark_unlinked(remote_id.0);
}

/// Sends the announce to every connected Link once something has changed.
///
/// Driven by the lobby's dirty flag rather than by comparing snapshots per Link: the events that
/// matter are "a Link appeared" and "the peer set changed", both of which set it, and the payload is
/// idempotent so an occasional redundant send costs nothing.
fn announce_changes(
    mut lobby: ResMut<Lobby>,
    mut links: Query<&mut MessageSender<LobbyAnnounce>, (With<P2P>, With<Connected>)>,
) {
    if !lobby.dirty {
        return;
    }
    let Some(announce) = lobby.announce() else {
        return;
    };
    let mut sent = false;
    for mut sender in &mut links {
        sender.send::<P2PChannel>(announce.clone());
        sent = true;
    }
    if sent {
        trace!(
            lobby = ?announce.lobby,
            known = announce.known.len(),
            "sent a lobby announce"
        );
        lobby.dirty = false;
    }
}

/// Asks the transport glue for a Link to every peer we know of but have not dialed.
fn dial_new_peers(mut commands: Commands, mut lobby: ResMut<Lobby>) {
    for peer in lobby.to_dial() {
        lobby.mark_dialed(peer);
        let context = lobby.dial_context(peer);
        debug!(?peer, "lobby dialing a peer");
        commands.trigger(DialPeer { peer, context });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn admission_app() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        app.insert_resource(pinned());
        app.add_observer(on_link_connected);
        app.add_observer(on_link_disconnected);
        app.add_observer(on_pending_disconnected);
        app.add_observer(on_pending_unlinked);
        app.add_observer(on_pending_disconnect);
        let endpoint = app.world_mut().spawn(LocalId(peer(0))).id();
        assert_eq!(
            resolve_endpoint_identity(app.world_mut(), endpoint, peer(9)),
            Some(peer(0))
        );
        (app, endpoint)
    }

    fn pending_link(world: &mut World, endpoint: Entity, expected: Option<PeerId>) -> Entity {
        let entity = world.spawn((P2P::Inactive, LinkOf { endpoint })).id();
        assert!(begin_peer_resolution(world, entity, endpoint, expected));
        entity
    }

    #[test]
    fn pending_links_publish_only_final_identity_at_admission() {
        let (mut app, endpoint) = admission_app();
        let entity = pending_link(app.world_mut(), endpoint, Some(peer(1)));
        assert!(app.world().get::<Connected>(entity).is_none());
        assert!(app.world().get::<LocalId>(entity).is_none());
        assert!(app.world().get::<RemoteId>(entity).is_none());
        assert!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .is_empty()
        );
        assert!(
            app.world()
                .resource::<Lobby>()
                .connected_members()
                .next()
                .is_none()
        );
        accept_link_announce(
            app.world_mut(),
            entity,
            announce_from(peer(1), b"direct", &[]),
        );
        assert!(app.world().get::<Connected>(entity).is_some());
        assert_eq!(app.world().get::<LocalId>(entity), Some(&LocalId(peer(0))));
        assert_eq!(
            app.world().get::<RemoteId>(entity),
            Some(&RemoteId(peer(1)))
        );
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&peer(1)),
            Some(&entity)
        );
        assert_eq!(
            app.world().resource::<Lobby>().roster(),
            vec![peer(0), peer(1)]
        );
        assert!(!begin_peer_resolution(
            app.world_mut(),
            entity,
            endpoint,
            Some(peer(2))
        ));
        assert_eq!(
            app.world().get::<RemoteId>(entity),
            Some(&RemoteId(peer(1)))
        );
    }

    #[test]
    fn colliding_pending_announcements_leave_incumbent_untouched() {
        let (mut app, endpoint) = admission_app();
        let first = pending_link(app.world_mut(), endpoint, None);
        let second = pending_link(app.world_mut(), endpoint, None);
        accept_link_announce(
            app.world_mut(),
            first,
            announce_from(peer(1), b"incumbent", &[]),
        );
        accept_link_announce(
            app.world_mut(),
            second,
            announce_from(peer(1), b"attacker", &[]),
        );
        assert!(app.world().get::<Connected>(first).is_some());
        assert!(app.world().get::<Connected>(second).is_none());
        assert!(app.world().get::<Disconnected>(second).is_some());
        assert!(app.world().get::<RemoteId>(second).is_none());
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&peer(1)),
            Some(&first)
        );
        assert_eq!(
            app.world()
                .resource::<Lobby>()
                .dial_context(peer(1))
                .as_ref(),
            b"incumbent"
        );
    }

    #[test]
    fn invalid_first_announcements_cannot_publish_membership() {
        for message in [
            announce_from(peer(2), b"", &[]),
            announce_from(peer(0), b"", &[]),
            announce_from(peer(1), b"", &[peer(2)]),
            announce_from(peer(1), &[0; MAX_DIAL_CONTEXT + 1], &[]),
        ] {
            let (mut app, endpoint) = admission_app();
            let entity = pending_link(app.world_mut(), endpoint, Some(peer(1)));
            accept_link_announce(app.world_mut(), entity, message);
            assert!(app.world().get::<Connected>(entity).is_none());
            assert!(app.world().get::<Disconnected>(entity).is_some());
            assert!(
                app.world()
                    .resource::<NetworkingMetadata>()
                    .peer_map
                    .is_empty()
            );
            assert_eq!(app.world().resource::<Lobby>().roster(), vec![peer(0)]);
            assert!(!begin_peer_resolution(
                app.world_mut(),
                entity,
                endpoint,
                Some(peer(1))
            ));
        }
    }

    #[test]
    fn later_announcements_cannot_rename_an_admitted_identity() {
        let (mut app, endpoint) = admission_app();
        let entity = pending_link(app.world_mut(), endpoint, None);
        accept_link_announce(
            app.world_mut(),
            entity,
            announce_from(peer(1), b"original", &[]),
        );
        accept_link_announce(
            app.world_mut(),
            entity,
            announce_from(peer(2), b"forged", &[peer(3)]),
        );
        assert_eq!(
            app.world().get::<RemoteId>(entity),
            Some(&RemoteId(peer(1)))
        );
        assert!(app.world().get::<Connected>(entity).is_none());
        assert!(app.world().get::<Disconnected>(entity).is_some());
        assert!(
            !app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&peer(2))
        );
        assert!(!app.world().resource::<Lobby>().peers.contains_key(&peer(2)));
        assert!(!app.world().resource::<Lobby>().peers.contains_key(&peer(3)));
    }

    #[test]
    fn endpoint_and_child_requests_cannot_split_the_local_identity() {
        let (mut app, endpoint) = admission_app();
        let conflicting = app
            .world_mut()
            .spawn((P2P::Inactive, LinkOf { endpoint }, LocalId(peer(2))))
            .id();
        assert!(!begin_peer_resolution(
            app.world_mut(),
            conflicting,
            endpoint,
            None
        ));
        assert!(app.world().get::<Disconnected>(conflicting).is_some());
        let wrong_remote = app
            .world_mut()
            .spawn((P2P::Inactive, LinkOf { endpoint }, RemoteId(peer(3))))
            .id();
        assert!(!begin_peer_resolution(
            app.world_mut(),
            wrong_remote,
            endpoint,
            Some(peer(1))
        ));
        let valid = app
            .world_mut()
            .spawn((
                P2P::Inactive,
                LinkOf { endpoint },
                LocalId(peer(0)),
                RemoteId(peer(1)),
            ))
            .id();
        assert!(begin_peer_resolution(
            app.world_mut(),
            valid,
            endpoint,
            Some(peer(1))
        ));
        assert!(app.world().get::<LocalId>(valid).is_none());
        let other_endpoint = app.world_mut().spawn(LocalId(peer(2))).id();
        assert_eq!(
            resolve_endpoint_identity(app.world_mut(), other_endpoint, peer(2)),
            None
        );
        assert_eq!(app.world().resource::<Lobby>().local(), Some(peer(0)));
        app.world_mut()
            .entity_mut(endpoint)
            .insert(LocalId(peer(3)));
        accept_link_announce(app.world_mut(), valid, announce_from(peer(1), b"", &[]));
        assert!(app.world().get::<Connected>(valid).is_none());
        assert!(app.world().get::<Disconnected>(valid).is_some());
    }

    #[test]
    fn resolution_respects_optout_and_missing_lobby_configuration() {
        let mut world = World::new();
        let endpoint = world.spawn_empty().id();
        let entity = world.spawn((P2P::Inactive, LinkOf { endpoint })).id();
        assert_eq!(
            resolve_endpoint_identity(&mut world, endpoint, peer(0)),
            None
        );
        assert!(!begin_peer_resolution(&mut world, entity, endpoint, None));
        assert!(world.get::<Connected>(entity).is_none());
        world.insert_resource(pinned());
        assert_eq!(
            resolve_endpoint_identity(&mut world, endpoint, peer(0)),
            Some(peer(0))
        );
        world.entity_mut(endpoint).insert(NoDefaultConnection);
        assert!(!begin_peer_resolution(&mut world, entity, endpoint, None));
        assert!(world.get::<PendingLobbyPeer>(entity).is_none());
        world.entity_mut(endpoint).remove::<NoDefaultConnection>();
        assert!(begin_peer_resolution(&mut world, entity, endpoint, None));
        assert!(begin_peer_resolution(&mut world, entity, endpoint, None));
        accept_link_announce(&mut world, entity, announce_from(peer(1), b"", &[]));
        assert_eq!(world.get::<RemoteId>(entity), Some(&RemoteId(peer(1))));
    }

    #[test]
    fn stopped_pending_attempts_do_not_publish_false_connectivity() {
        let (mut app, endpoint) = admission_app();
        let entity = pending_link(app.world_mut(), endpoint, None);
        app.world_mut().trigger(Disconnect { entity });
        app.world_mut().flush();
        accept_link_announce(app.world_mut(), entity, announce_from(peer(1), b"", &[]));
        assert!(app.world().get::<Connected>(entity).is_none());
        assert!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .is_empty()
        );
        assert!(!app.world().resource::<Lobby>().is_connected(peer(1)));
        assert!(!begin_peer_resolution(
            app.world_mut(),
            entity,
            endpoint,
            None
        ));
    }

    #[test]
    fn obsolete_disconnect_cannot_remove_replacement_binding() {
        let (mut app, endpoint) = admission_app();
        let obsolete = pending_link(app.world_mut(), endpoint, None);
        accept_link_announce(app.world_mut(), obsolete, announce_from(peer(1), b"", &[]));
        app.world_mut().entity_mut(obsolete).remove::<Connected>();
        let replacement = pending_link(app.world_mut(), endpoint, None);
        accept_link_announce(
            app.world_mut(),
            replacement,
            announce_from(peer(1), b"", &[]),
        );
        app.world_mut()
            .entity_mut(obsolete)
            .insert(Disconnected::default());
        app.world_mut().flush();
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&peer(1)),
            Some(&replacement)
        );
        assert!(app.world().resource::<Lobby>().is_connected(peer(1)));
        app.world_mut()
            .entity_mut(replacement)
            .insert(Disconnected::default());
        app.world_mut().flush();
        assert!(!app.world().resource::<Lobby>().is_connected(peer(1)));
    }

    #[test]
    fn direct_context_updates_do_not_change_identity_or_accept_third_party_overrides() {
        let (mut app, endpoint) = admission_app();
        let direct = pending_link(app.world_mut(), endpoint, None);
        let third_party = pending_link(app.world_mut(), endpoint, None);
        accept_link_announce(app.world_mut(), direct, announce_from(peer(1), b"", &[]));
        accept_link_announce(
            app.world_mut(),
            third_party,
            announce_from(peer(2), b"", &[]),
        );
        let mut forged = announce_from(peer(2), b"", &[]);
        forged.known.push((peer(1), Bytes::from_static(b"forged")));
        accept_link_announce(app.world_mut(), third_party, forged);
        assert!(
            app.world()
                .resource::<Lobby>()
                .dial_context(peer(1))
                .is_empty()
        );
        accept_link_announce(
            app.world_mut(),
            direct,
            announce_from(peer(1), b"updated", &[]),
        );
        assert_eq!(
            app.world()
                .resource::<Lobby>()
                .dial_context(peer(1))
                .as_ref(),
            b"updated"
        );
        assert_eq!(
            app.world().get::<RemoteId>(direct),
            Some(&RemoteId(peer(1)))
        );
        assert_eq!(
            app.world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&peer(1)),
            Some(&direct)
        );
    }

    /// A stand-in peer. The lobby never inspects a `PeerId`, so any variant does; `Entity` is what
    /// the transports that address peers by an index use.
    fn peer(n: u8) -> PeerId {
        PeerId::Entity(u64::from(n))
    }

    fn id() -> LobbyId {
        LobbyId::from_bytes([1; 32])
    }

    fn other_id() -> LobbyId {
        LobbyId::from_bytes([2; 32])
    }

    /// A lobby this peer opened, or one an invite pinned.
    fn pinned() -> Lobby {
        Lobby::new(LobbyIdPolicy::Pinned(Some(id())))
    }

    /// A lobby with no id at all.
    fn open() -> Lobby {
        Lobby::new(LobbyIdPolicy::Pinned(None))
    }

    /// A lobby that has not been told which gathering it is in.
    fn adopting() -> Lobby {
        Lobby::new(LobbyIdPolicy::Adopt)
    }

    fn known_ids(announce: &LobbyAnnounce) -> Vec<PeerId> {
        announce.known.iter().map(|(peer, _)| *peer).collect()
    }

    /// An announce that names the peers it knows and publishes no dial context at all.
    ///
    /// A real [`Lobby::announce`] also names *itself* so peers can dial it back; tests about
    /// dial contexts use [`announce_from`] for that, and tests about membership do not care.
    fn announce(lobby: Option<LobbyId>, known: &[PeerId]) -> LobbyAnnounce {
        LobbyAnnounce {
            lobby,
            from: peer(9),
            known: known.iter().map(|peer| (*peer, Bytes::new())).collect(),
        }
    }

    /// An announce in which the sender names itself and says how to dial it.
    fn announce_from(sender: PeerId, context: &[u8], known: &[PeerId]) -> LobbyAnnounce {
        let mut entries: SmallVec<[(PeerId, DialContext); 5]> =
            known.iter().map(|peer| (*peer, Bytes::new())).collect();
        entries.push((sender, Bytes::copy_from_slice(context)));
        LobbyAnnounce {
            lobby: Some(id()),
            from: sender,
            known: entries,
        }
    }

    #[test]
    fn a_joiner_adopts_the_first_lobby_it_hears() {
        let mut lobby = adopting();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        assert_eq!(lobby.id(), None, "an adopting lobby starts with no id");

        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        assert_eq!(lobby.id(), Some(id()));
        assert_eq!(lobby.members().collect::<Vec<_>>(), vec![peer(1)]);
    }

    #[test]
    fn a_joiner_that_has_not_settled_is_accepted_by_a_pinned_lobby() {
        // The invite case, in order: the joiner announces `None` first, and must not be mistaken
        // for a peer that disagrees.
        let mut founder = pinned();
        founder.mark_local(peer(0));
        founder.mark_linked(peer(1));
        founder.accept_announce(peer(1), announce(None, &[]));

        assert!(founder.is_member(peer(1)), "an unsettled peer is admitted");
        assert_eq!(founder.roster(), vec![peer(0), peer(1)]);
    }

    #[test]
    fn a_peer_in_another_lobby_is_excluded_and_not_advertised() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        lobby.accept_announce(peer(2), announce(Some(other_id()), &[]));

        assert_eq!(lobby.members().collect::<Vec<_>>(), vec![peer(1)]);
        assert!(!lobby.is_member(peer(2)));
        assert_eq!(
            lobby.peers().find(|(p, _)| *p == peer(2)).map(|(_, s)| s),
            Some(PeerState::Foreign)
        );
        assert!(
            !lobby.known().iter().any(|(known, _)| *known == peer(2)),
            "a foreign peer must not be spread to the rest of the lobby"
        );
        assert_eq!(lobby.roster(), vec![peer(0), peer(1)]);
    }

    #[test]
    fn a_lobby_without_an_id_accepts_every_peer() {
        let mut lobby = open();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(1), announce(None, &[]));
        lobby.accept_announce(peer(2), announce(Some(other_id()), &[]));

        assert_eq!(lobby.id(), None, "an open lobby never acquires an id");
        assert_eq!(
            lobby.members().collect::<Vec<_>>(),
            vec![peer(1), peer(2)],
            "and it has nothing to disagree with, so nobody is foreign"
        );
        assert_eq!(lobby.roster(), vec![peer(0), peer(1), peer(2)]);
    }

    #[test]
    fn a_pinned_lobby_keeps_its_id_when_a_peer_disagrees() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(9));
        lobby.accept_announce(peer(9), announce(Some(other_id()), &[]));

        assert_eq!(lobby.id(), Some(id()), "the pinned id is not adopted over");
        assert!(lobby.members().next().is_none());
        assert_eq!(lobby.roster(), vec![peer(0)], "only ourselves");
    }

    #[test]
    fn an_adopting_lobby_settles_once() {
        let mut lobby = adopting();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        // A peer from another lobby cannot change what we already settled on.
        lobby.accept_announce(peer(2), announce(Some(other_id()), &[]));

        assert_eq!(lobby.id(), Some(id()));
        assert!(!lobby.is_member(peer(2)));
    }

    #[test]
    fn the_roster_is_sorted_so_slots_agree_without_coordination() {
        // A sees itself, 1 and 2; B sees itself, 0 and 2. Both must derive the same roster, and the
        // same slot for a given peer, without exchanging anything but membership.
        let mut a = pinned();
        a.mark_local(peer(0));
        a.mark_linked(peer(1));
        a.mark_linked(peer(2));
        a.accept_announce(peer(1), announce(Some(id()), &[peer(2)]));
        a.accept_announce(peer(2), announce(Some(id()), &[]));

        let mut b = pinned();
        b.mark_local(peer(1));
        b.mark_linked(peer(0));
        b.mark_linked(peer(2));
        b.accept_announce(peer(0), announce(Some(id()), &[peer(2)]));
        b.accept_announce(peer(2), announce(Some(id()), &[]));

        assert_eq!(a.roster(), b.roster());
        assert_eq!(a.roster(), vec![peer(0), peer(1), peer(2)]);
        for peer in [peer(0), peer(1), peer(2)] {
            assert_eq!(a.slot_of(peer), b.slot_of(peer));
        }
        assert_eq!(a.local_slot(), Some(0));
        assert_eq!(b.local_slot(), Some(1));
    }

    #[test]
    fn a_peer_learned_from_an_announce_is_dialed_once() {
        let mut lobby = pinned();
        // peer(1) is connected (it announced over a Link); peer(2) is only known.
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[peer(2)]));

        assert_eq!(
            lobby.to_dial(),
            vec![peer(2)],
            "only the peer we have no Link to is dialed"
        );
        for peer in lobby.to_dial() {
            lobby.mark_dialed(peer);
        }
        assert!(lobby.to_dial().is_empty(), "a peer is never dialed twice");
    }

    #[test]
    fn a_bootstrap_peer_is_dialed_before_anything_is_connected() {
        let mut lobby = adopting();
        lobby.add_bootstrap([peer(3)]);
        assert_eq!(lobby.waiting().collect::<Vec<_>>(), vec![peer(3)]);
        assert_eq!(lobby.to_dial(), vec![peer(3)]);
    }

    #[test]
    fn a_foreign_peer_is_dialed_before_it_turns_out_to_be_foreign() {
        // Membership is only known once a peer answers, so it is dialed first: discovery is
        // optimistic and the announce is what corrects it.
        let mut lobby = pinned();
        lobby.add_bootstrap([peer(3)]);
        assert_eq!(lobby.to_dial(), vec![peer(3)]);

        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(3));
        lobby.accept_announce(peer(3), announce(Some(other_id()), &[]));

        assert!(!lobby.is_member(peer(3)));
        assert!(lobby.to_dial().is_empty(), "and it is not dialed again");
        assert!(
            lobby.is_connected(peer(3)),
            "the Link is left alone: what to do with a connected foreign peer is the \
             application's decision"
        );
    }

    #[test]
    fn connectivity_is_reported_separately_from_membership() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        lobby.accept_announce(peer(2), announce(Some(id()), &[]));

        assert_eq!(lobby.connected_members().count(), 2);

        // peer(2)'s Link goes away: it is still a known member, but no longer connected.
        lobby.mark_unlinked(peer(2));
        assert!(lobby.is_member(peer(2)), "membership outlives a Link");
        assert!(!lobby.is_connected(peer(2)));
        assert_eq!(lobby.connected_members().collect::<Vec<_>>(), vec![peer(1)]);
    }

    #[test]
    fn an_announce_does_not_clear_connectivity() {
        // Connectivity lives in a set beside the peer entry, so changing the *state* must not
        // touch it. Replacing the entry wholesale would drop what `linked` still records.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        assert!(lobby.is_connected(peer(1)));
        assert!(!lobby.is_member(peer(1)));

        lobby.accept_announce(peer(1), announce(Some(id()), &[]));

        assert!(lobby.is_member(peer(1)));
        assert!(lobby.is_connected(peer(1)), "the Link did not go away");
        assert_eq!(lobby.connected_members().count(), 1);
    }

    #[test]
    fn losing_a_link_does_not_warrant_an_announce() {
        // An announce says who is in the lobby and which peers we know. A Link going away changes
        // neither, so re-announcing would send every peer an identical snapshot.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        lobby.dirty = false;

        lobby.mark_unlinked(peer(1));

        assert!(!lobby.is_connected(peer(1)));
        assert!(lobby.is_member(peer(1)), "membership outlives the Link");
        assert!(!lobby.dirty, "nothing to say about it");
    }

    #[test]
    fn a_failed_dial_is_not_retried_until_asked() {
        // A dial is attempted once, so an unreachable peer stays unreachable until the application
        // decides to try again. Without this the lobby would either give up permanently or dial in
        // a loop; neither is the lobby's call.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.add_bootstrap([peer(1)]);
        for peer in lobby.to_dial() {
            lobby.mark_dialed(peer);
        }
        assert!(lobby.to_dial().is_empty(), "already attempted");

        // The attempt failed. Nothing happens on its own...
        assert_eq!(lobby.retry_failed(), 1, "...until asked");
        assert_eq!(lobby.to_dial(), vec![peer(1)]);
    }

    #[test]
    fn retry_ignores_a_peer_we_are_already_connected_to() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        assert!(!lobby.retry(peer(1)), "there is nothing to retry");
        assert_eq!(lobby.retry_failed(), 0);
        assert!(lobby.to_dial().is_empty());
    }

    #[test]
    fn a_disconnected_inbound_member_waits_for_explicit_recovery() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        lobby.mark_unlinked(peer(1));

        // Being the lower id does not authorize an automatic retry of an accepted connection.
        assert!(lobby.to_dial().is_empty());
        assert!(lobby.is_member(peer(1)));
        assert_eq!(lobby.retry_failed(), 1);
        assert_eq!(lobby.retry_failed(), 0);
        assert!(!lobby.retry(peer(1)));
        assert_eq!(lobby.to_dial(), vec![peer(1)]);
        assert!(lobby.is_member(peer(1)));

        lobby.mark_dialed(peer(1));
        lobby.mark_linked(peer(1));
        assert!(!lobby.retry(peer(1)));
        assert_eq!(lobby.retry_failed(), 0);
        lobby.mark_unlinked(peer(1));
        assert!(
            lobby.retry(peer(1)),
            "a later disconnection can be re-armed"
        );
        assert!(!lobby.retry(peer(1)));
        assert_eq!(lobby.retry_failed(), 0);
        assert_eq!(lobby.to_dial(), vec![peer(1)]);
    }

    #[test]
    fn retry_bypasses_the_tie_break_even_after_an_automatic_dial() {
        let mut lobby = pinned();
        lobby.learn(peer(1), Bytes::from_static(b"peer-key"));
        // With no local id yet, the initial automatic dial is allowed.
        assert_eq!(lobby.to_dial(), vec![peer(1)]);
        lobby.mark_dialed(peer(1));
        lobby.mark_local(peer(2));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        lobby.mark_unlinked(peer(1));

        assert!(lobby.retry(peer(1)));
        assert!(!lobby.retry(peer(1)));
        assert_eq!(lobby.retry_failed(), 0);
        assert_eq!(lobby.to_dial(), vec![peer(1)]);
        assert!(lobby.is_member(peer(1)));
        assert_eq!(lobby.dial_context(peer(1)).as_ref(), b"peer-key");
    }

    #[test]
    fn the_lower_id_dials_a_peer_learned_second_hand() {
        // Two peers that hear of each other at the same moment must not both dial: a transport
        // where dialing and accepting create different entities would end up with two Links to one
        // peer, and P2PStart rejects a duplicate identity outright.
        let mut lower = pinned();
        lower.mark_local(peer(1));
        lower.mark_linked(peer(0));
        lower.accept_announce(peer(0), announce(Some(id()), &[peer(2)]));
        assert_eq!(lower.to_dial(), vec![peer(2)], "the lower id dials");

        let mut higher = pinned();
        higher.mark_local(peer(2));
        higher.mark_linked(peer(0));
        higher.accept_announce(peer(0), announce(Some(id()), &[peer(1)]));
        assert!(
            higher.to_dial().is_empty(),
            "the higher id waits to be dialed instead"
        );
    }

    #[test]
    fn a_peer_the_application_asked_for_is_dialed_whatever_the_ids_say() {
        // An invitee has to dial its host even when its own id is higher: the host may not know the
        // invitee exists, so waiting to be dialed would stall forever.
        let mut lobby = pinned();
        lobby.mark_local(peer(5));
        lobby.add_bootstrap([peer(1)]);
        assert_eq!(lobby.to_dial(), vec![peer(1)]);
    }

    #[test]
    fn the_tie_break_leaves_a_skipped_peer_retryable() {
        // The tie-break cannot cover the case where the other side never learns of us: it holds the
        // dial back, and nothing ever dials. The peer stays `Wanted` and was never dialed, so
        // asking for it is the recovery.
        let mut lobby = pinned();
        lobby.mark_local(peer(2));
        lobby.mark_linked(peer(0));
        lobby.accept_announce(peer(0), announce(Some(id()), &[peer(1)]));
        assert!(lobby.to_dial().is_empty(), "held back by the tie-break");

        assert!(lobby.retry_failed() == 1, "the skipped peer is re-armable");
        assert_eq!(lobby.to_dial(), vec![peer(1)]);

        // And it is only reported once: a second call has nothing new to re-arm.
        lobby.mark_dialed(peer(1));
        assert_eq!(lobby.retry_failed(), 1, "a dial that was attempted");
    }

    #[test]
    fn a_dial_context_travels_with_the_peer_that_announced_it() {
        // The lobby carries the transport's bytes without reading them, and hands them back when
        // the peer is dialed.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce_from(peer(1), b"peer-one-key", &[]));

        assert_eq!(
            lobby.dial_context(peer(1)).as_ref(),
            b"peer-one-key",
            "the sender names itself, so its own context is kept for re-dialing it"
        );

        // We do the same, so peers can dial us.
        lobby.set_dial_context(Bytes::from_static(b"our-key"));
        let announce = lobby.announce().expect("we know our own id");
        let own = announce
            .known
            .iter()
            .find(|(announced, _)| *announced == peer(0))
            .expect("we name ourselves");
        assert_eq!(own.1.as_ref(), b"our-key");
    }

    #[test]
    fn an_announce_hop_passes_along_how_to_reach_the_peer_it_introduces() {
        // Peer 0 is the only one holding peer 2's context, so it has to relay it: peer 1 cannot
        // dial a peer it has no key for.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(2), announce_from(peer(2), b"peer-two-key", &[]));

        let announce = lobby.announce().expect("we know our own id");
        let relayed = announce
            .known
            .iter()
            .find(|(announced, _)| *announced == peer(2))
            .expect("peer 2 is announced");
        assert_eq!(
            relayed.1.as_ref(),
            b"peer-two-key",
            "the introduced peer's context is relayed, not just its id"
        );
    }

    #[test]
    fn an_oversized_context_is_discarded_rather_than_truncated() {
        // The bytes come from an untrusted peer. A transport would misread a partial key, so the
        // lobby drops it entirely and lets the dial fail cleanly.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(
            peer(1),
            announce_from(peer(1), &[7u8; MAX_DIAL_CONTEXT + 1], &[]),
        );

        assert!(lobby.dial_context(peer(1)).is_empty());
    }

    #[test]
    fn an_empty_context_never_replaces_one_we_already_have() {
        // The peer that introduces two others may hold a key where a later announce does not, and
        // losing it would strand a dial.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.add_bootstrap_with_context([(peer(1), Bytes::from_static(b"key"))]);
        assert_eq!(lobby.dial_context(peer(1)).as_ref(), b"key");

        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(2), announce(Some(id()), &[peer(1)]));
        assert_eq!(
            lobby.dial_context(peer(1)).as_ref(),
            b"key",
            "an announce with no context must not erase the key we were given"
        );
    }

    #[test]
    fn a_linked_but_unconfirmed_peer_can_be_retried() {
        // A connectionless transport reports a Link as soon as it is dialed, with nothing to confirm
        // that anyone is listening. Such a peer is still `Wanted`, so a retry is still meaningful —
        // this is the case a `!linked` test would silently skip.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.add_bootstrap([peer(1)]);
        for peer in lobby.to_dial() {
            lobby.mark_dialed(peer);
        }
        lobby.mark_linked(peer(1));
        assert!(!lobby.is_member(peer(1)), "nothing has confirmed the peer");
        lobby.dirty = false;

        assert!(
            lobby.retry(peer(1)),
            "...so retrying it still means something"
        );
        // There is already a Link, so retrying cannot produce another dial. What it does is make the
        // lobby speak again, which is how a connectionless transport is actually recovered.
        assert!(lobby.dirty, "...and it re-announces");
        assert!(lobby.to_dial().is_empty(), "the Link is already there");
        assert!(!lobby.retry(peer(1)));
        assert_eq!(lobby.retry_failed(), 0);

        // Once it announces, it is a member and there is nothing left to retry.
        lobby.mark_dialed(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        assert!(lobby.is_member(peer(1)));
        assert!(!lobby.retry(peer(1)));
        assert_eq!(lobby.retry_failed(), 0);
    }

    #[test]
    fn a_peer_in_another_lobby_is_never_dialed_again() {
        // We learn a peer is elsewhere only by connecting, so this is the re-dial case: once known
        // to be foreign, its Link dropping must not make us try again.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(other_id()), &[]));
        lobby.mark_dialed(peer(1));
        lobby.mark_unlinked(peer(1));

        assert!(!lobby.retry(peer(1)), "not ours to retry");
        assert_eq!(lobby.retry_failed(), 0);
        assert!(lobby.to_dial().is_empty(), "and never dialed again");
    }

    #[test]
    fn forgetting_a_peer_also_forgets_its_dial() {
        // This is what the flags-on-the-entry shape buys: a peer's dial state cannot outlive the
        // peer, so there is no second collection to keep in step and no way to forget one and not
        // the other.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.add_bootstrap([peer(1)]);
        for peer in lobby.to_dial() {
            lobby.mark_dialed(peer);
        }
        assert!(lobby.retry_failed() == 1, "the failed dial is re-armable");

        // Forget the peer without ever having linked it, then bring it back into view.
        lobby.forget_disconnected();
        assert!(lobby.peers().next().is_none(), "nothing is remembered");
        assert_eq!(lobby.retry_failed(), 0, "and no dial state lingers");
        assert!(lobby.to_dial().is_empty());

        // A forgotten peer that reappears is a fresh peer, so it is dialed again.
        lobby.add_bootstrap([peer(1)]);
        assert_eq!(lobby.to_dial(), vec![peer(1)]);
    }

    #[test]
    fn forgetting_drops_peers_that_are_gone() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        lobby.accept_announce(peer(2), announce(Some(id()), &[]));
        lobby.mark_dialed(peer(2));
        assert_eq!(lobby.roster().len(), 3);

        // peer(2) leaves.
        lobby.mark_unlinked(peer(2));
        assert_eq!(lobby.forget_disconnected(), 1);

        assert_eq!(lobby.roster(), vec![peer(0), peer(1)]);
        assert!(!lobby.is_member(peer(2)));
        assert!(
            !lobby.known().iter().any(|(known, _)| *known == peer(2)),
            "a forgotten peer is no longer advertised"
        );
        assert!(
            lobby.to_dial().is_empty(),
            "and is not re-dialed behind the application's back"
        );
        assert_eq!(lobby.forget_disconnected(), 0, "nothing left to forget");
    }

    #[test]
    fn forgetting_is_effective_for_the_other_peers() {
        // The point of forgetting is that the remaining peers stop hearing about the departed one,
        // so the announce must change.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        lobby.accept_announce(peer(2), announce(Some(id()), &[]));
        lobby.mark_unlinked(peer(2));
        lobby.forget_disconnected();

        let announce = lobby.announce().expect("we know our own id");
        let announced = known_ids(&announce);
        assert!(
            announced.contains(&peer(1)),
            "the remaining peer is still named"
        );
        assert!(!announced.contains(&peer(2)), "the forgotten peer is not");
        assert!(announced.contains(&peer(0)), "and we name ourselves");
    }

    #[test]
    fn a_connected_peer_is_never_forgotten() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        assert_eq!(lobby.forget_disconnected(), 0);
        assert!(lobby.is_member(peer(1)));
    }

    #[test]
    fn the_local_peer_is_never_learned_or_dialed() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        // Our own id arriving in someone's announce must not make us dial ourselves.
        lobby.accept_announce(peer(1), announce(Some(id()), &[peer(0)]));

        assert!(!lobby.peers().any(|(p, _)| p == peer(0)));
        assert!(!lobby.to_dial().contains(&peer(0)));
    }

    /// Proves the "never dial ourselves" invariant is maintained by [`Lobby::learn`] refusing our
    /// id and [`Lobby::observe`] purging it — not by a check in `to_dial`, which no longer has one.
    #[test]
    fn a_peer_never_learns_or_dials_itself() {
        // The regression this guards: a remote announce legitimately contains *our* id (the peer
        // knows us). If that is processed before we have learned our own id from a Link, we record
        // ourselves as `Wanted` and then try to dial ourselves, which the transport rejects.
        let mut lobby = pinned();
        assert_eq!(lobby.local(), None, "no Link has been seen yet");
        lobby.accept_announce(peer(1), announce(Some(id()), &[peer(0)]));

        // peer(0) is us, but the lobby cannot know that yet, so it is recorded. The guard that
        // matters is the one in `dial_new_peers`, which runs only after `observe_links` has had the
        // chance to set `local`.
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        assert_eq!(lobby.local(), Some(peer(0)));
        assert!(
            !lobby.to_dial().contains(&peer(0)),
            "and we are not a dial target once our id is known"
        );
        assert!(!lobby.peers().any(|(p, _)| p == peer(0)));
    }

    #[test]
    fn an_announce_never_replaces_a_known_peer_with_a_worse_state() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        assert!(lobby.is_member(peer(1)));

        // A third party tells us about peer(1) as an unknown peer. That must not demote it.
        lobby.accept_announce(peer(2), announce(Some(id()), &[peer(1)]));
        assert!(lobby.is_member(peer(1)));
    }

    #[test]
    fn the_announce_lists_members_and_pending_peers_only() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.mark_linked(peer(2));
        lobby.accept_announce(peer(1), announce(Some(id()), &[peer(3)]));
        lobby.accept_announce(peer(2), announce(Some(other_id()), &[]));

        let announce = lobby.announce().expect("we know our own id");
        assert_eq!(announce.lobby, Some(id()));
        let announced = known_ids(&announce);
        assert!(announced.contains(&peer(1)), "members are announced");
        assert!(announced.contains(&peer(3)), "pending peers are announced");
        assert!(!announced.contains(&peer(2)), "a foreign peer is not");
    }

    #[test]
    fn only_real_changes_make_the_lobby_dirty() {
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        assert!(
            lobby.dirty,
            "an adopting lobby starts with something to say"
        );

        // Settle, then write off the initial announcement.
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.dirty = false;

        // Nothing new: the lobby must stay quiet, or it would announce every frame.
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        assert!(lobby.dirty, "a peer becoming a member is news");

        lobby.dirty = false;
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(1));
        lobby.accept_announce(peer(1), announce(Some(id()), &[]));
        assert!(!lobby.dirty, "repeating the same announce is not");
    }
}
