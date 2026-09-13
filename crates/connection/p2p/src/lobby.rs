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
//! Peers exchange [`LobbyAnnounce`]s over Links they already have, on the same
//! [`P2PChannel`](crate::session::P2PChannel) the session handshake uses. Each announce is a full
//! snapshot of everything the sender knows, so the protocol is idempotent and order-free.
//! A peer unions what it receives and emits [`DialPeer`] to establish a [`Link`] to peers it
//! isn't connected to yet.
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
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageReceiver, MessageSender};
use bytes::Bytes;
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

/// Sent on every connected peer Link to describe the sender's lobby and the peers it knows.
///
/// A full snapshot, not a delta, which is what makes the protocol idempotent and order-free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyAnnounce {
    /// The sender's lobby, or `None` if it has not settled on one.
    pub lobby: Option<LobbyId>,
    /// Who sent this.
    ///
    /// A receiver can usually infer this from the Link it arrived on, and for a datagram transport
    /// that is exact: the peer's identity *is* the address its datagrams come from. A stream
    /// transport cannot do that — an accepted session reports the ephemeral port the peer dialed
    /// from, not the endpoint it listens on — so the sender names itself and the receiver re-keys
    /// the Link. See [`Lobby::rename_peer`].
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
///    what `lightyear_raw_connection` does: it tags each peer `PeerId::Raw(peer_addr)`.
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
    /// Whether we have already asked the transport to dial this peer.
    dialed: bool,
    /// How to dial this peer.
    context: DialContext,
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
            dial_explicitly: false,
        }
    }
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
    /// The local peer's id, read from any Link's [`LocalId`].
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
        if self.dial_context == context {
            return;
        }
        self.dial_context = context;
        self.dirty = true;
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
    /// A dial is attempted once, so a peer that was unreachable when we first tried stays
    /// unreachable. This re-arms it, so the lobby emits [`DialPeer`] for it again on the next
    /// frame.
    ///
    /// Returns `false` when there is nothing to retry — the peer is unknown, already confirmed, or
    /// known to be in another lobby. Retrying is the application's decision and its pacing: the
    /// lobby has no clock, and a peer that is simply slow must not be dialed in a loop.
    ///
    /// A peer that is [`PeerState::Wanted`] is retryable even while a Link exists, because a Link is
    /// not evidence that anyone is listening: a connectionless transport reports one as soon as it
    /// is dialed. Reaching such a peer is exactly what a retry is for.
    ///
    /// Re-arming also marks the lobby dirty. On a transport where dialing carries an announce, the
    /// announce that went with the failed dial is as likely to be what was lost as the dial itself,
    /// so a retry has to speak again rather than only repeat a transport call.
    pub fn retry(&mut self, peer: PeerId) -> bool {
        let Some(info) = self.peers.get_mut(&peer) else {
            return false;
        };
        if info.state == PeerState::Foreign || (info.linked && info.state != PeerState::Wanted) {
            return false;
        }
        // Already dialed: re-arming makes the lobby speak again, which is what recovers a
        // connectionless transport whose announce went missing.
        if core::mem::replace(&mut info.dialed, false) {
            self.dirty = true;
            return true;
        }
        // Connected without us dialing: an inbound Link with nothing to retry.
        if info.linked || info.dial_explicitly {
            return false;
        }
        // Never dialed and not connected: the tie-break may be what held it back, and the peer may
        // never dial us in turn. Asking for it settles the question.
        info.dial_explicitly = true;
        true
    }

    /// Asks for every peer we wanted but could not reach to be dialed again.
    ///
    /// Returns how many were re-armed. This is the recovery call: after an invite's peer was not
    /// up yet, or after a Link dropped, this is what makes the lobby try again.
    pub fn retry_failed(&mut self) -> usize {
        let mut rearmed = 0;
        for info in self.peers.values_mut() {
            if info.state != PeerState::Wanted {
                continue;
            }
            // Connected without us dialing: nothing to retry.
            if info.linked && !info.dialed {
                continue;
            }
            let was_dialed = core::mem::replace(&mut info.dialed, false);
            // A peer we never dialed is one the tie-break left to the other side, which may never
            // dial either; asking for it is the whole point of a retry.
            let newly_explicit = !info.dial_explicitly;
            info.dial_explicitly = true;
            if was_dialed || (newly_explicit && !info.linked) {
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
    fn mark_local(&mut self, local: PeerId) {
        if self.local == Some(local) {
            return;
        }
        self.local = Some(local);
        if self.peers.remove(&local).is_some() {
            self.dirty = true;
        }
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
            info.linked = false;
        }
    }

    /// Moves everything we know about `old` to `new`, returning whether there was anything to move.
    ///
    /// A stream transport reports the ephemeral source address of an accepted session, so the first
    /// announce to arrive on it is the only thing that can say which peer is actually there. This is
    /// what carries that correction through: the link keeps its identity, and so do the membership
    /// and dial state gathered under it.
    ///
    /// Returns `false` for an unknown `old`, or when `new` is already the local peer — either way
    /// there is nothing to move.
    pub fn rename_peer(&mut self, old: PeerId, new: PeerId) -> bool {
        if old == new || self.local == Some(new) {
            return false;
        }
        let Some(info) = self.peers.remove(&old) else {
            return false;
        };
        match self.peers.entry(new) {
            // Keep whatever the known entry already has: it may have been learned from an announce
            // and so carry a context this provisional entry lacks.
            Entry::Occupied(mut slot) => {
                let existing = slot.get_mut();
                existing.linked |= info.linked;
                existing.dialed |= info.dialed;
                existing.dial_explicitly |= info.dial_explicitly;
                if existing.context.is_empty() {
                    existing.context = info.context;
                }
                if existing.state == PeerState::Wanted && info.state != PeerState::Wanted {
                    existing.state = info.state;
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(info);
            }
        }
        self.dirty = true;
        true
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
                if info.context.is_empty() && !context.is_empty() {
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
        // The sender is in `known` like any other peer, so its own context arrives here too.
        for (peer, context) in announce.known {
            self.learn(peer, bounded_context(context));
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
        app.add_systems(PreUpdate, drain_announcements.after(MessageSystems::Receive));
        app.add_systems(PostUpdate, (announce_changes, dial_new_peers));
    }
}

/// Unions every announce received this frame.
fn drain_announcements(
    mut commands: Commands,
    mut lobby: ResMut<Lobby>,
    mut links: Query<(Entity, &RemoteId, &mut MessageReceiver<LobbyAnnounce>), With<P2P>>,
) {
    for (entity, remote_id, mut receiver) in &mut links {
        // `RemoteId` is immutable, so the correction is written through commands and carried here
        // for the rest of this frame's announces.
        let mut effective = remote_id.0;
        for message in receiver.receive() {
            // The sender is the authority on its own identity. Usually the Link already agrees, and
            // for a datagram transport it always does; a stream transport sees the ephemeral port
            // the peer dialed from, so this is where the link learns which peer it holds.
            if message.from != effective && lobby.rename_peer(effective, message.from) {
                debug!(
                    was = ?effective,
                    now = ?message.from,
                    "re-keying a Link to the id the peer named itself"
                );
                effective = message.from;
                commands.entity(entity).insert(RemoteId(effective));
            }
            lobby.accept_announce(effective, message);
        }
    }
}

/// Records a Link coming up: the peer is linked, and we now know our own id.
fn on_link_connected(
    trigger: On<Add, Connected>,
    links: Query<(&RemoteId, &LocalId), With<P2P>>,
    mut lobby: ResMut<Lobby>,
) {
    let Ok((remote_id, local_id)) = links.get(trigger.entity) else {
        return;
    };
    lobby.mark_local(local_id.0);
    lobby.mark_linked(remote_id.0);
}

/// Records a Link going away.
fn on_link_disconnected(
    trigger: On<Remove, Connected>,
    links: Query<&RemoteId, With<P2P>>,
    mut lobby: ResMut<Lobby>,
) {
    let Ok(remote_id) = links.get(trigger.entity) else {
        return;
    };
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
    fn a_rename_moves_membership_and_dial_state_to_the_named_id() {
        // A stream transport reports the ephemeral port an accepted session dialed from, so the
        // first announce on it is the only thing that can say which peer is really there. Everything
        // gathered under the provisional id has to survive the correction.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.mark_linked(peer(50));
        lobby.accept_announce(peer(50), announce(Some(id()), &[]));
        assert!(lobby.is_member(peer(50)));

        assert!(lobby.rename_peer(peer(50), peer(2)));
        assert!(!lobby.is_member(peer(50)), "the provisional id is gone");
        assert!(lobby.is_member(peer(2)), "the named id inherits membership");
        assert!(lobby.is_connected(peer(2)), "and the Link");
        assert!(lobby.roster().contains(&peer(2)));
    }

    #[test]
    fn a_rename_keeps_a_context_the_named_peer_already_had() {
        // A peer can be known from an announce — with a context — before its own announce arrives
        // over the session it opened. The correction must not throw that away.
        let mut lobby = pinned();
        lobby.mark_local(peer(0));
        lobby.add_bootstrap_with_context([(peer(2), Bytes::from_static(b"named-key"))]);
        lobby.mark_linked(peer(50));

        assert!(lobby.rename_peer(peer(50), peer(2)));
        assert_eq!(
            lobby.dial_context(peer(2)).as_ref(),
            b"named-key",
            "the key learned from another peer survives the correction"
        );
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

        assert!(lobby.retry(peer(1)), "...so retrying it still means something");
        // There is already a Link, so retrying cannot produce another dial. What it does is make the
        // lobby speak again, which is how a connectionless transport is actually recovered.
        assert!(lobby.dirty, "...and it re-announces");
        assert!(lobby.to_dial().is_empty(), "the Link is already there");

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
        assert!(announced.contains(&peer(1)), "the remaining peer is still named");
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
        assert!(lobby.dirty, "an adopting lobby starts with something to say");

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
