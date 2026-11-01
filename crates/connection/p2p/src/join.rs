//! Joining a running deterministic session.
//!
//! "Your app" below means the game built on lightyear, as opposed to this framework. The join
//! handshake crosses that boundary four times, always as local events; everything on the wire
//! is private:
//!
//! 1. Your app triggers [`P2PJoin`] on the newcomer, naming one remote peer.
//! 2. The framework sends a private request message to that remote peer.
//! 3. The framework triggers [`P2PJoinRequested`] locally on the remote peer; your app there
//!    answers with [`P2PJoinAdmission`]. The request is not forwarded to other members.
//! 4. An accepted admission is announced to the existing roster so its Links can stream inputs
//!    to the newcomer. Once those Links are ready, the newcomer catches up through
//!    [`P2PJoinCatchUp`].
//!
//! After [`P2PJoinCatchUpComplete`], the newcomer proposes one future activation tick to every
//! incumbent. All must acknowledge the same proposal before it is announced. Membership and
//! player creation change together at that tick through [`P2PJoined`]. There is no separate
//! membership/commit tick.
//!
//! This is a coordinated session protocol, not fault-tolerant consensus: an activation announcement
//! must reach every participant before its tick. A connection failure after announcement requires
//! session-level recovery, not unilateral cancellation of an already agreed activation.

use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use bevy_ecs::system::SystemParam;
use bevy_time::{Real, Time};
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_connection::p2p::{P2P, P2PRoster, P2PSessionPhase};
use lightyear_core::id::{LocalId, PeerId};
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_messages::prelude::{AppTriggerExt, EventSender, RemoteEvent};
#[cfg(test)]
use lightyear_sync::prelude::SyncedLocalTimeline;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{P2PChannel, P2PSession};

/// Join another peer's running session. Trigger this on the newcomer.
///
/// The named remote peer must already be reachable: discovery and dialing are your app's
/// responsibility (for example through the [`Lobby`](crate::Lobby)), and a Link identifying
/// that peer must exist. The local session must be [`Stopped`](crate::P2PSessionState::Stopped);
/// a peer already in a session cannot join another one.
///
/// What happens next, all driven by the framework:
/// 1. A request is sent to the remote peer, whose app answers it (see [`P2PJoinRequested`]).
/// 2. On acceptance the newcomer links up with every member and catches up on their history.
/// 3. [`P2PJoined`] fires on all members at the activation tick; the newcomer's player is
///    created there, before that tick simulates.
///
/// If any step fails, [`P2PJoinRejected`] fires locally with the reason, and the peer returns
/// to a clean stopped state it can retry from.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoin {
    /// The running session member to ask. Its app decides on admission.
    pub remote_peer: PeerId,
}

/// The remote peer's app is asked whether `peer_id` may join. Local to that peer.
///
/// This is not the wire message (which is private); it is the framework asking your app for a
/// decision. Answer with [`P2PJoinAdmission`], immediately or within the negotiation timeout.
/// There is no implicit acceptance: an unanswered request times out and is refused.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinRequested {
    /// The newcomer asking to join.
    pub peer_id: PeerId,
}

/// Your app's answer to one [`P2PJoinRequested`].
///
/// Only the peer and the decision are needed; the framework matches the pending admission and
/// fills in the protocol identifiers when replying. Answers for unknown peers, or answers that
/// arrive after the admission moved on (a decision was already sent, the request timed out, or
/// the peer was admitted), are silently ignored.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinAdmission {
    /// Must match the [`P2PJoinRequested::peer_id`] being answered.
    pub peer_id: PeerId,
    /// Admit with `Ok(())`, or refuse with a reason.
    pub result: Result<(), JoinRejectReason>,
}

/// Why admission, catch-up, or activation negotiation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinRejectReason {
    /// The remote peer is not in a started session.
    NotStarted,
    /// The remote peer is already admitting someone else, or is itself joining.
    Busy,
    /// The remote peer's app refused admission.
    Refused,
    /// No answer, no Links, or no acknowledgements within the negotiation timeout.
    Timeout,
    /// A message from an older membership generation arrived after the roster moved on.
    StaleEpoch,
    /// The activation tick passed before all acknowledgements arrived.
    TooLate,
    /// Catch-up itself failed (history unavailable or divergent).
    CatchUpFailed,
    /// Not all roster Links were usable when catch-up needed them.
    RosterIncomplete,
}

/// A new member joins at `activate_tick`. Create its player now, on every peer.
///
/// In the live session this fires exactly once per member, on the tick *before* the
/// activation tick (before the timeline increments to it), so anything spawned here exists
/// before that tick simulates. Membership and player creation change together here; there is
/// no separate commit step.
///
/// - On the newcomer (`local_is_joiner`), spawn the local player. `link` is `None`: there is
///   no single input-receiving Link for your own inputs.
/// - On every incumbent, `link` is the input-receiving Link from the newcomer. Spawn its
///   remote representation and wire input routing through that Link.
///
/// Catch-up re-emits historical activations while a newcomer replays, so observers also fire
/// for past joins there; write them to be idempotent per (`peer_id`, `activate_tick`).
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoined {
    /// The member joining the session.
    pub peer_id: PeerId,
    /// First tick the member simulates as part of the session.
    pub activate_tick: Tick,
    /// True on the newcomer itself, false on the incumbents.
    pub local_is_joiner: bool,
    /// Input-receiving Link on incumbents; `None` on the newcomer itself.
    pub link: Option<Entity>,
}

/// All incumbent input Links are ready. Supply the joining app's world state.
///
/// Input-only apps reconstruct the initial world and replay its input archive. Snapshot apps
/// restore a snapshot instead. Catch-up owns its own progress budget and ends by triggering
/// [`P2PJoinCatchUpComplete`] or [`P2PJoinCancel`].
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinCatchUp {
    pub epoch: u32,
    pub session_start_tick: Option<Tick>,
    /// Admission coordinator, also the natural source for the catch-up state.
    pub remote_peer: PeerId,
}

/// Catch-up finished locally; negotiate a single future activation tick.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinCatchUpComplete {
    /// Last tick simulated with complete remote input coverage.
    pub caught_up_tick: Tick,
    /// Oldest restorable state, used as this newcomer's input rollback floor.
    pub history_start_tick: Tick,
}

/// Cancel a local join before its activation has been announced.
///
/// Trigger this to abort your own attempt (or let the framework trigger it on timeouts and
/// refusals). Cancellation notifies the contacted peers and ends with [`P2PJoinRejected`].
/// Once an activation is announced it can no longer be cancelled; that needs session recovery.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinCancel {
    pub reason: JoinRejectReason,
}

/// A local join failed. Your app may return to its lobby or trigger [`P2PJoin`] to try again.
///
/// This is the outward notice; [`P2PJoinCancel`] is the inward trigger that produces it.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinRejected {
    pub reason: JoinRejectReason,
}

/// Join-specific session bookkeeping. A single instance is owned by [`P2PSession`].
#[derive(Debug, Default, Clone)]
pub(crate) struct P2PJoinState {
    pub(crate) epoch: u32,
    next_request: u32,
    /// Last cancelled attempt per remote peer; unordered control traffic may arrive after it.
    aborted: SmallVec<[(u32, JoinId); 4]>,
    pub(crate) attempt: Option<JoinAttempt>,
    pub(crate) admission: Option<HostAdmission>,
    pub(crate) activation: Option<PendingActivation>,
}

impl P2PJoinState {
    /// Whether uid=501(charles) gid=20(staff) groups=20(staff),12(everyone),61(localaccounts),79(_appserverusr),80(admin),81(_appserveradm),701(com.apple.sharepoint.group.1),702(com.apple.sharepoint.group.2),33(_appstore),98(_lpadmin),100(_lpoperator),204(_developer),250(_analyticsusers),395(com.apple.access_ftp),398(com.apple.access_screensharing),399(com.apple.access_ssh),400(com.apple.access_remote_ae) is covered by a remembered abort in the current epoch.
    fn was_aborted(&self, id: JoinId) -> bool {
        self.aborted.iter().any(|(epoch, previous)| {
            *epoch == self.epoch
                && previous.peer == id.peer
                && (id.request.wrapping_sub(previous.request) as i32) <= 0
        })
    }

    fn remember_abort(&mut self, id: JoinId) {
        if self.was_aborted(id) {
            return;
        }
        if let Some(previous) = self
            .aborted
            .iter_mut()
            .find(|(_, previous)| previous.peer == id.peer)
        {
            *previous = (self.epoch, id);
        } else {
            self.aborted.push((self.epoch, id));
        }
    }

    pub(crate) fn clear(&mut self) {
        self.attempt = None;
        self.admission = None;
        self.activation = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct JoinId {
    peer: PeerId,
    request: u32,
}

/// Newcomer side of one join attempt, from request to announced activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinStage {
    /// Request sent to the remote peer; awaiting its [`JoinResponse`].
    Requested,
    /// Roster known; waiting for a connected Link to every host plus the roster-ready notice.
    Connecting,
    /// Links are up; the catch-up plugin is replaying history (`P2PJoinCatchUp` in flight).
    CatchingUp,
    /// Caught up through the contained tick; about to propose it (plus start delay) as the
    /// activation tick.
    Ready(Tick),
    /// Proposal sent; collecting one [`JoinActivationAck`] per host for the contained tick.
    Proposed(Tick),
    /// Every host acknowledged; the activation announcement is sent and the tick is reserved.
    Announced,
}

#[derive(Debug, Clone)]
pub(crate) struct JoinAttempt {
    id: JoinId,
    remote_peer: PeerId,
    pub(crate) hosts: SmallVec<[PeerId; 4]>,
    pub(crate) session_start_tick: Option<Tick>,
    stage: JoinStage,
    started_at: Duration,
    roster_ready_epoch: Option<u32>,
    acks: SmallVec<[PeerId; 4]>,
}

/// Incumbent side of one admission, from request to reserved activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionStage {
    /// The app has not answered [`P2PJoinRequested`] yet. Only the contacted remote peer (the
    /// coordinator) ever sits here; other members learn of the admission already accepted.
    AwaitingDecision,
    /// Accepted; waiting for a connected Link to the newcomer.
    Connecting,
    /// Own Link is up. Non-coordinating members stop here; the coordinator additionally waits
    /// for every member's Link report before telling the newcomer the roster is ready.
    Ready,
    /// Reserved the newcomer's proposed activation tick; awaiting its announcement.
    Proposed(Tick),
}

#[derive(Debug, Clone)]
pub(crate) struct HostAdmission {
    id: JoinId,
    coordinator: PeerId,
    epoch: u32,
    stage: AdmissionStage,
    started_at: Duration,
    /// Only the coordinator collects these; other hosts acknowledge their own Link once.
    acks: SmallVec<[PeerId; 4]>,
    ready_sent: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingActivation {
    pub(crate) peer_id: PeerId,
    pub(crate) epoch: u32,
    pub(crate) activate_tick: Tick,
    pub(crate) local_is_joiner: bool,
}

#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinRequest {
    request: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JoinRoster {
    epoch: u32,
    peers: SmallVec<[PeerId; 4]>,
    start_tick: Tick,
}

#[derive(Event, Debug, Clone, Serialize, Deserialize)]
struct JoinResponse {
    request: u32,
    result: Result<JoinRoster, JoinRejectReason>,
}

/// Notification of the coordinator's accepted decision to the other members. Not another
/// app-level request: receiving members skip straight to watching their Link.
#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinAdmitted {
    id: JoinId,
    epoch: u32,
}

#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinLinkReady {
    id: JoinId,
    epoch: u32,
    result: Result<(), JoinRejectReason>,
}

#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinRosterReady {
    id: JoinId,
    epoch: u32,
}

#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinActivationProposal {
    id: JoinId,
    epoch: u32,
    tick: Tick,
}

#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinActivationAck {
    proposal: JoinActivationProposal,
    result: Result<(), JoinRejectReason>,
}

#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinActivate {
    proposal: JoinActivationProposal,
}

#[derive(Event, Debug, Clone, Copy, Serialize, Deserialize)]
struct JoinAbort {
    id: JoinId,
    epoch: u32,
    reason: JoinRejectReason,
}

/// Registers the same wire events on every application before Links are constructed.
#[doc(hidden)]
pub struct P2PJoinProtocolPlugin;

impl Plugin for P2PJoinProtocolPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::P2PProtocolPlugin>() {
            app.add_plugins(crate::P2PProtocolPlugin);
        }
        app.register_event::<JoinRequest>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinResponse>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinAdmitted>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinLinkReady>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinRosterReady>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinActivationProposal>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinActivationAck>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinActivate>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<JoinAbort>()
            .add_direction(NetworkDirection::Bidirectional);
    }
}

/// Installed automatically by [`P2PSessionPlugin`](crate::P2PSessionPlugin); requires its
/// [`P2PSession`](crate::P2PSession) resource, so never add this standalone.
#[doc(hidden)]
pub struct P2PJoinPlugin;

impl Plugin for P2PJoinPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<P2PJoinProtocolPlugin>() {
            app.add_plugins(P2PJoinProtocolPlugin);
        }
        // The per-frame and per-tick drivers live in the session plugin's own systems
        // (`drive_session`, `start_session_before_agreed_tick`): both pairs share the session
        // resource, so separate systems would be ambiguous.
        app.add_observer(begin_join)
            .add_observer(receive_request)
            .add_observer(answer_request)
            .add_observer(receive_response)
            .add_observer(receive_admission)
            .add_observer(receive_link_ready)
            .add_observer(receive_roster_ready)
            .add_observer(on_catch_up_complete)
            .add_observer(receive_proposal)
            .add_observer(receive_activation_ack)
            .add_observer(receive_activation)
            .add_observer(cancel_join)
            .add_observer(abort_admission)
            .add_observer(receive_abort);
    }
}

/// Membership and sender state the join driver reads alongside the session.
///
/// Bundled so the session plugin's driver can carry it without naming the join
/// protocol's event types.
#[derive(SystemParam)]
pub(crate) struct JoinDriveParams<'w, 's> {
    metadata: Res<'w, NetworkingMetadata>,
    links: Query<'w, 's, (&'static P2P, Has<Connected>)>,
    link_ready: Query<'w, 's, &'static mut EventSender<JoinLinkReady>>,
    roster_ready: Query<'w, 's, &'static mut EventSender<JoinRosterReady>>,
    proposals: Query<'w, 's, &'static mut EventSender<JoinActivationProposal>>,
    activations: Query<'w, 's, &'static mut EventSender<JoinActivate>>,
}

/// Send one event on the Link to `peer`. Returns false when no sender exists.
fn trigger_peer<E: Event>(
    peer: PeerId,
    event: E,
    metadata: &NetworkingMetadata,
    senders: &mut Query<&mut EventSender<E>>,
) -> bool {
    let Some(&link) = metadata.peer_map.get(&peer) else {
        return false;
    };
    let Ok(mut sender) = senders.get_mut(link) else {
        return false;
    };
    sender.trigger::<P2PChannel>(event);
    true
}

/// Start a join attempt: send the request to the remote peer and track it.
///
/// Only a stopped peer with an inactive Link to the remote peer may join.
fn begin_join(
    trigger: On<P2PJoin>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
    metadata: Res<NetworkingMetadata>,
    links: Query<(&P2P, &LocalId)>,
    mut senders: Query<&mut EventSender<JoinRequest>>,
) {
    if session.is_joining() || !matches!(session.state(), crate::P2PSessionState::Stopped) {
        return;
    }
    let Some(&link) = metadata.peer_map.get(&trigger.remote_peer) else {
        return;
    };
    let Ok((state, local)) = links.get(link) else {
        return;
    };
    if *state != P2P::Inactive {
        return;
    }
    session.observe_local_id(local.0);
    session.join.next_request = session.join.next_request.wrapping_add(1);
    let request = session.join.next_request;
    if !trigger_peer(
        trigger.remote_peer,
        JoinRequest { request },
        &metadata,
        &mut senders,
    ) {
        return;
    }
    session.join.attempt = Some(JoinAttempt {
        id: JoinId {
            peer: local.0,
            request,
        },
        remote_peer: trigger.remote_peer,
        hosts: SmallVec::new(),
        session_start_tick: None,
        stage: JoinStage::Requested,
        started_at: time.elapsed(),
        roster_ready_epoch: None,
        acks: SmallVec::new(),
    });
    commands.entity(link).insert(P2P::Candidate);
    tracing::info!(remote_peer = ?trigger.remote_peer, request, "asking to join a running P2P session");
}

/// Handle an incoming join request: refuse when not started or busy, else store the
/// admission and ask the app through [`P2PJoinRequested`].
fn receive_request(
    trigger: On<RemoteEvent<JoinRequest>>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
    metadata: Res<NetworkingMetadata>,
    links: Query<(&P2P, &LocalId), With<Connected>>,
    mut senders: Query<&mut EventSender<JoinResponse>>,
) {
    let Some(&link) = metadata.peer_map.get(&trigger.from) else {
        return;
    };
    let Ok((state, local)) = links.get(link) else {
        return;
    };
    session.observe_local_id(local.0);
    let id = JoinId {
        peer: trigger.from,
        request: trigger.trigger.request,
    };
    if session.join.was_aborted(id) {
        return;
    }
    let reason = if !session.is_started() {
        Some(JoinRejectReason::NotStarted)
    } else if *state == P2P::Joined
        || session.join.admission.is_some()
        || session.join.activation.is_some()
    {
        Some(JoinRejectReason::Busy)
    } else {
        None
    };
    if let Some(reason) = reason {
        trigger_peer(
            trigger.from,
            JoinResponse {
                request: trigger.trigger.request,
                result: Err(reason),
            },
            &metadata,
            &mut senders,
        );
        return;
    }
    let epoch = session.epoch();
    session.join.admission = Some(HostAdmission {
        id,
        coordinator: local.0,
        epoch,
        stage: AdmissionStage::AwaitingDecision,
        started_at: time.elapsed(),
        acks: SmallVec::new(),
        ready_sent: false,
    });
    commands.trigger(P2PJoinRequested { peer_id: id.peer });
}

/// Apply the app's admission decision: refuse the newcomer, or announce the acceptance
/// to the roster and send it back with the session roster.
fn answer_request(
    trigger: On<P2PJoinAdmission>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    metadata: Res<NetworkingMetadata>,
    mut responses: Query<&mut EventSender<JoinResponse>>,
    mut notices: Query<&mut EventSender<JoinAdmitted>>,
) {
    let Some(admission) = session.join.admission.as_ref() else {
        return;
    };
    // At most one admission is ever pending (concurrent requests are refused as busy), so the
    // peer plus the awaiting stage identify the answer's request; its identifiers stay here.
    if admission.id.peer != trigger.peer_id || admission.stage != AdmissionStage::AwaitingDecision {
        return;
    }
    let id = admission.id;
    let epoch = admission.epoch;
    let local = admission.coordinator;
    let result = if session.is_started() {
        trigger.result
    } else {
        Err(JoinRejectReason::NotStarted)
    };
    if let Err(reason) = result {
        trigger_peer(
            id.peer,
            JoinResponse {
                request: id.request,
                result: Err(reason),
            },
            &metadata,
            &mut responses,
        );
        session.join.admission = None;
        return;
    }
    let mut peers = session.started_peers();
    for &peer in &peers {
        trigger_peer(peer, JoinAdmitted { id, epoch }, &metadata, &mut notices);
    }
    peers.push(local);
    peers.sort_unstable();
    let start_tick = session
        .start_tick()
        .expect("started session has a founding tick");
    trigger_peer(
        id.peer,
        JoinResponse {
            request: id.request,
            result: Ok(JoinRoster {
                epoch,
                peers,
                start_tick,
            }),
        },
        &metadata,
        &mut responses,
    );
    session.join.admission.as_mut().unwrap().stage = AdmissionStage::Connecting;
    if let Some(&link) = metadata.peer_map.get(&id.peer) {
        commands.entity(link).insert(P2P::Candidate);
    }
}

/// Handle the remote peer's answer: adopt its roster and start connecting, or cancel.
fn receive_response(
    trigger: On<RemoteEvent<JoinResponse>>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
) {
    let Some(attempt) = session.join.attempt.as_ref() else {
        return;
    };
    if trigger.from != attempt.remote_peer
        || trigger.trigger.request != attempt.id.request
        || attempt.stage != JoinStage::Requested
    {
        return;
    }
    let roster = match &trigger.trigger.result {
        Ok(roster) => roster,
        Err(reason) => {
            commands.trigger(P2PJoinCancel { reason: *reason });
            return;
        }
    };
    if !roster.peers.contains(&attempt.remote_peer)
        || roster.peers.contains(&attempt.id.peer)
        || roster
            .peers
            .iter()
            .enumerate()
            .any(|(i, peer)| roster.peers[..i].contains(peer))
    {
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::StaleEpoch,
        });
        return;
    }
    session.join.epoch = roster.epoch;
    let attempt = session.join.attempt.as_mut().unwrap();
    attempt.hosts.clone_from(&roster.peers);
    attempt.session_start_tick = Some(roster.start_tick);
    attempt.stage = JoinStage::Connecting;
}

/// Handle a coordinator's accepted announcement: track the admission from `Connecting`.
fn receive_admission(
    trigger: On<RemoteEvent<JoinAdmitted>>,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
    metadata: Res<NetworkingMetadata>,
    mut senders: Query<&mut EventSender<JoinLinkReady>>,
) {
    let message = trigger.trigger;
    if !session.is_started() {
        return;
    }
    let peers = session.started_peers();
    if !peers.contains(&trigger.from) {
        return;
    }
    if session.join.was_aborted(message.id) {
        return;
    }
    let reason = if message.epoch != session.epoch() {
        Some(JoinRejectReason::StaleEpoch)
    } else if session.join.admission.is_some()
        || session.join.activation.is_some()
        || peers.contains(&message.id.peer)
        || session.local_peer_id() == Some(message.id.peer)
    {
        Some(JoinRejectReason::Busy)
    } else {
        None
    };
    if let Some(reason) = reason {
        trigger_peer(
            trigger.from,
            JoinLinkReady {
                id: message.id,
                epoch: message.epoch,
                result: Err(reason),
            },
            &metadata,
            &mut senders,
        );
        return;
    }
    session.join.admission = Some(HostAdmission {
        id: message.id,
        coordinator: trigger.from,
        epoch: message.epoch,
        stage: AdmissionStage::Connecting,
        started_at: time.elapsed(),
        acks: SmallVec::new(),
        ready_sent: false,
    });
}

/// Collect one member's Link report on the coordinator; abort the admission on error.
fn receive_link_ready(
    trigger: On<RemoteEvent<JoinLinkReady>>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
) {
    if !session.started_peers().contains(&trigger.from) {
        return;
    }
    let local = session.local_peer_id();
    let Some(admission) = session.join.admission.as_mut() else {
        return;
    };
    if Some(admission.coordinator) != local
        || admission.id != trigger.trigger.id
        || admission.epoch != trigger.trigger.epoch
    {
        return;
    }
    if let Err(reason) = trigger.trigger.result {
        commands.trigger(AbortAdmission { reason });
    } else if !admission.acks.contains(&trigger.from) {
        admission.acks.push(trigger.from);
    }
}

/// Record the coordinator's all-Links-ready notice on the newcomer.
fn receive_roster_ready(
    trigger: On<RemoteEvent<JoinRosterReady>>,
    mut session: ResMut<P2PSession>,
) {
    let Some(attempt) = session.join.attempt.as_mut() else {
        return;
    };
    if trigger.from == attempt.remote_peer && trigger.trigger.id == attempt.id {
        attempt.roster_ready_epoch = Some(trigger.trigger.epoch);
    }
}

/// Move the attempt to `Ready` with the tick catch-up covered.
fn on_catch_up_complete(
    trigger: On<P2PJoinCatchUpComplete>,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
) {
    let Some(attempt) = session.join.attempt.as_mut() else {
        return;
    };
    if attempt.stage != JoinStage::CatchingUp {
        return;
    }
    attempt.stage = JoinStage::Ready(trigger.caught_up_tick);
    attempt.started_at = time.elapsed();
    tracing::info!(caught_up_tick = ?trigger.caught_up_tick, history_start_tick = ?trigger.history_start_tick, "P2P catch-up complete");
}

/// Answer one activation proposal: reserve its tick, or refuse it when late or busy.
fn receive_proposal(
    trigger: On<RemoteEvent<JoinActivationProposal>>,
    mut session: ResMut<P2PSession>,
    timeline: Res<LocalTimeline>,
    metadata: Res<NetworkingMetadata>,
    mut senders: Query<&mut EventSender<JoinActivationAck>>,
) {
    let proposal = trigger.trigger;
    let Some(admission) = session.join.admission.as_mut() else {
        return;
    };
    if trigger.from != admission.id.peer
        || proposal.id != admission.id
        || proposal.epoch != admission.epoch
    {
        return;
    }
    let result = if proposal.tick <= timeline.tick() {
        Err(JoinRejectReason::TooLate)
    } else if admission.stage != AdmissionStage::Ready {
        Err(JoinRejectReason::Busy)
    } else {
        admission.stage = AdmissionStage::Proposed(proposal.tick);
        Ok(())
    };
    trigger_peer(
        trigger.from,
        JoinActivationAck { proposal, result },
        &metadata,
        &mut senders,
    );
}

/// Collect one proposal acknowledgement; cancel the attempt on refusal.
fn receive_activation_ack(
    trigger: On<RemoteEvent<JoinActivationAck>>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
) {
    let proposal = trigger.trigger.proposal;
    let epoch = session.epoch();
    let Some(attempt) = session.join.attempt.as_mut() else {
        return;
    };
    if proposal.id != attempt.id
        || proposal.epoch != epoch
        || attempt.stage != JoinStage::Proposed(proposal.tick)
        || !attempt.hosts.contains(&trigger.from)
    {
        return;
    }
    if let Err(reason) = trigger.trigger.result {
        commands.trigger(P2PJoinCancel { reason });
    } else if !attempt.acks.contains(&trigger.from) {
        attempt.acks.push(trigger.from);
    }
}

/// Reserve a fully-acknowledged activation tick announced by the newcomer.
fn receive_activation(
    trigger: On<RemoteEvent<JoinActivate>>,
    mut session: ResMut<P2PSession>,
    timeline: Res<LocalTimeline>,
) {
    let proposal = trigger.trigger.proposal;
    let Some(admission) = session.join.admission.as_ref() else {
        return;
    };
    if trigger.from != admission.id.peer
        || proposal.id != admission.id
        || proposal.epoch != admission.epoch
        || admission.stage != AdmissionStage::Proposed(proposal.tick)
    {
        return;
    }
    if proposal.tick <= timeline.tick() {
        tracing::error!(?proposal, local_tick = ?timeline.tick(), "P2P activation announcement missed its boundary; session recovery required");
        return;
    }
    session.join.activation = Some(PendingActivation {
        peer_id: proposal.id.peer,
        epoch: proposal.epoch,
        activate_tick: proposal.tick,
        local_is_joiner: false,
    });
}

#[derive(Event)]
struct AbortAdmission {
    reason: JoinRejectReason,
}

/// Drop a host-side admission and notify the newcomer (plus the roster, if coordinator).
fn abort_admission(
    trigger: On<AbortAdmission>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    metadata: Res<NetworkingMetadata>,
    mut senders: Query<&mut EventSender<JoinAbort>>,
) {
    if session.join.activation.is_some() {
        return;
    }
    let Some(admission) = session.join.admission.take() else {
        return;
    };
    session.join.remember_abort(admission.id);
    let event = JoinAbort {
        id: admission.id,
        epoch: admission.epoch,
        reason: trigger.reason,
    };
    trigger_peer(admission.id.peer, event, &metadata, &mut senders);
    if session.local_peer_id() == Some(admission.coordinator) {
        for peer in session.started_peers() {
            trigger_peer(peer, event, &metadata, &mut senders);
        }
    }
    if let Some(&link) = metadata.peer_map.get(&admission.id.peer) {
        commands.entity(link).insert(P2P::Inactive);
    }
}

/// Drop a newcomer-side attempt, notify the contacted peers, and tell the app it failed.
fn cancel_join(
    trigger: On<P2PJoinCancel>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    metadata: Res<NetworkingMetadata>,
    mut senders: Query<&mut EventSender<JoinAbort>>,
) {
    if session.join.activation.is_some() {
        return;
    }
    let epoch = session.epoch();
    let Some(attempt) = session.join.attempt.take() else {
        return;
    };
    let event = JoinAbort {
        id: attempt.id,
        epoch,
        reason: trigger.reason,
    };
    // Before the response arrives, only the remote peer is known.
    for peer in core::iter::once(attempt.remote_peer).chain(
        attempt
            .hosts
            .into_iter()
            .filter(|p| *p != attempt.remote_peer),
    ) {
        trigger_peer(peer, event, &metadata, &mut senders);
        if let Some(&link) = metadata.peer_map.get(&peer) {
            commands.entity(link).insert(P2P::Inactive);
        }
    }
    commands.trigger(P2PJoinRejected {
        reason: trigger.reason,
    });
}

/// Route an abort notice to the matching attempt or admission, remembering it against
/// reordered traffic that may arrive after the local state is gone.
fn receive_abort(
    trigger: On<RemoteEvent<JoinAbort>>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    metadata: Res<NetworkingMetadata>,
) {
    let message = trigger.trigger;
    // An abort can beat the admission notification across different Links. Remember it even
    // without a pending admission, but only for the sender itself or an existing coordinator.
    if session.is_started()
        && session.join.activation.is_none()
        && metadata.peer_map.contains_key(&trigger.from)
        && (trigger.from == message.id.peer
            || (message.epoch == session.epoch()
                && session.started_peers().contains(&trigger.from)))
    {
        session.join.remember_abort(message.id);
    }
    if let Some(attempt) = session.join.attempt.as_ref()
        && message.id == attempt.id
        && (trigger.from == attempt.remote_peer || attempt.hosts.contains(&trigger.from))
    {
        commands.trigger(P2PJoinCancel {
            reason: message.reason,
        });
    } else if let Some(admission) = session.join.admission.as_ref()
        && message.id == admission.id
        // The newcomer may cancel before receiving the roster and learning its epoch.
        && (trigger.from == admission.id.peer
            || (trigger.from == admission.coordinator && message.epoch == admission.epoch))
    {
        commands.trigger(AbortAdmission {
            reason: message.reason,
        });
    }
}

/// Advance join admission and attempts once per frame after message receipt.
///
/// Called from [`drive_session`](crate::session::drive_session) rather than running as its own
/// system: both drive the session resource, so separate systems would be ambiguous.
pub(crate) fn drive_join_inner(
    commands: &mut Commands,
    session: &mut P2PSession,
    time: &Time<Real>,
    timeline: &LocalTimeline,
    synced: bool,
    mut params: JoinDriveParams,
) {
    if session.join.admission.is_none() && session.join.attempt.is_none() {
        return;
    }
    let local = session.local_peer_id();
    let timeout = session.start_timeout();
    let host_count = session.started_peers().len();
    if let Some(admission) = session.join.admission.as_mut() {
        if matches!(
            admission.stage,
            AdmissionStage::AwaitingDecision | AdmissionStage::Connecting
        ) && time.elapsed().saturating_sub(admission.started_at) >= timeout
        {
            commands.trigger(AbortAdmission {
                reason: JoinRejectReason::Timeout,
            });
        } else if admission.stage == AdmissionStage::Connecting
            && let Some(&link) = params.metadata.peer_map.get(&admission.id.peer)
            && let Ok((state, connected)) = params.links.get(link)
            && connected
        {
            if *state == P2P::Inactive {
                commands.entity(link).insert(P2P::Candidate);
            }
            admission.stage = AdmissionStage::Ready;
            if Some(admission.coordinator) != local {
                trigger_peer(
                    admission.coordinator,
                    JoinLinkReady {
                        id: admission.id,
                        epoch: admission.epoch,
                        result: Ok(()),
                    },
                    &params.metadata,
                    &mut params.link_ready,
                );
            }
        }
        if admission.stage == AdmissionStage::Ready
            && Some(admission.coordinator) == local
            && admission.acks.len() == host_count
            && !admission.ready_sent
        {
            admission.ready_sent = trigger_peer(
                admission.id.peer,
                JoinRosterReady {
                    id: admission.id,
                    epoch: admission.epoch,
                },
                &params.metadata,
                &mut params.roster_ready,
            );
        }
    }
    let epoch = session.epoch();
    let delay = i32::from(session.start_delay_ticks());
    let Some(attempt) = session.join.attempt.as_mut() else {
        return;
    };
    if !matches!(attempt.stage, JoinStage::CatchingUp | JoinStage::Announced)
        && time.elapsed().saturating_sub(attempt.started_at) >= timeout
    {
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::Timeout,
        });
        return;
    }
    match attempt.stage {
        JoinStage::Requested | JoinStage::CatchingUp | JoinStage::Announced => {}
        JoinStage::Connecting => {
            let mut all_connected = true;
            for peer in &attempt.hosts {
                let Some(&link) = params.metadata.peer_map.get(peer) else {
                    all_connected = false;
                    continue;
                };
                let Ok((state, connected)) = params.links.get(link) else {
                    all_connected = false;
                    continue;
                };
                if *state == P2P::Inactive {
                    commands.entity(link).insert(P2P::Candidate);
                }
                all_connected &= connected;
            }
            if all_connected && attempt.roster_ready_epoch == Some(epoch) && synced {
                attempt.stage = JoinStage::CatchingUp;
                commands.trigger(P2PJoinCatchUp {
                    epoch,
                    session_start_tick: attempt.session_start_tick,
                    remote_peer: attempt.remote_peer,
                });
            }
        }
        JoinStage::Ready(caught_up) => {
            let tick = caught_up.max(timeline.tick()) + delay;
            let proposal = JoinActivationProposal {
                id: attempt.id,
                epoch,
                tick,
            };
            for &peer in &attempt.hosts {
                trigger_peer(peer, proposal, &params.metadata, &mut params.proposals);
            }
            attempt.stage = JoinStage::Proposed(tick);
            tracing::info!(?tick, ?caught_up, "proposing P2P activation after catch-up");
        }
        JoinStage::Proposed(tick) => {
            if tick <= timeline.tick() {
                commands.trigger(P2PJoinCancel {
                    reason: JoinRejectReason::TooLate,
                });
                return;
            }
            if attempt.acks.len() != attempt.hosts.len() {
                return;
            }
            let proposal = JoinActivationProposal {
                id: attempt.id,
                epoch,
                tick,
            };
            for &peer in &attempt.hosts {
                trigger_peer(
                    peer,
                    JoinActivate { proposal },
                    &params.metadata,
                    &mut params.activations,
                );
            }
            attempt.stage = JoinStage::Announced;
            session.join.activation = Some(PendingActivation {
                peer_id: proposal.id.peer,
                epoch,
                activate_tick: tick,
                local_is_joiner: true,
            });
        }
    }
}

/// Exercise the join driver through the scheduler in tests.
///
/// Production calls [`drive_join_inner`] from the session driver instead.
#[cfg(test)]
fn drive_join(
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
    timeline: Res<LocalTimeline>,
    synced: Option<SyncedLocalTimeline>,
    params: JoinDriveParams,
) {
    drive_join_inner(
        &mut commands,
        &mut session,
        &time,
        &timeline,
        synced.is_some(),
        params,
    );
}

/// Apply a scheduled membership change at its activation tick.
///
/// Called from
/// [`start_session_before_agreed_tick`](crate::session::start_session_before_agreed_tick) rather
/// than running as its own system: both drive the session resource, so separate systems would be
/// ambiguous.
pub(crate) fn apply_join_activation_inner(
    commands: &mut Commands,
    session: &mut P2PSession,
    timeline: &LocalTimeline,
    metadata: &NetworkingMetadata,
    phase: &mut P2PSessionPhase,
) {
    let Some(activation) = session.join.activation else {
        return;
    };
    if timeline.tick() + 1 != activation.activate_tick {
        return;
    }
    let joiner_link = metadata.peer_map.get(&activation.peer_id).copied();
    if activation.local_is_joiner {
        let attempt = session
            .join
            .attempt
            .as_ref()
            .expect("local activation has an attempt");
        for peer in &attempt.hosts {
            if let Some(&link) = metadata.peer_map.get(peer) {
                commands.entity(link).insert(P2P::Joined);
            }
        }
    } else if let Some(link) = joiner_link {
        commands.entity(link).insert(P2P::Joined);
    }
    session.apply_activation(activation);
    *phase = P2PSessionPhase::Active;
    // Publish the projection after the immutable component replacements, before the player
    // observer and fixed step. Waiting for PostUpdate would expose stale input membership.
    commands.queue(move |world: &mut World| {
        let roster = P2PRoster::from_links(
            P2PSessionPhase::Active,
            world.query::<(Entity, &P2P, Has<Connected>)>().iter(world)
                .map(|(entity, state, connected)| (entity, *state, connected)),
        );
        world.resource_mut::<NetworkingMetadata>().mode = NetworkTopology::P2P(roster);
        tracing::info!(peer = ?activation.peer_id, activate_tick = ?activation.activate_tick,
            local_is_joiner = activation.local_is_joiner, "P2P join activated; the newcomer is a player");
        world.trigger(P2PJoined {
            peer_id: activation.peer_id, activate_tick: activation.activate_tick,
            local_is_joiner: activation.local_is_joiner,
            link: if activation.local_is_joiner { None } else { joiner_link },
        });
    });
}

/// Exercise the activation driver through the scheduler in tests.
///
/// Production calls [`apply_join_activation_inner`] from the session driver instead.
#[cfg(test)]
fn apply_join_activation(
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    timeline: Res<LocalTimeline>,
    metadata: Res<NetworkingMetadata>,
    mut phase: ResMut<P2PSessionPhase>,
) {
    apply_join_activation_inner(
        &mut commands,
        &mut session,
        &timeline,
        &metadata,
        &mut phase,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use bevy_ecs::system::RunSystemOnce;
    use lightyear_core::id::RemoteId;
    use lightyear_sync::prelude::LocalTimelineSync;

    fn peer(id: u64) -> PeerId {
        PeerId::Raw(core::net::SocketAddr::from(([127, 0, 0, 1], id as u16)))
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<P2PSession>();
        app.init_resource::<NetworkingMetadata>();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<P2PSessionPhase>();
        app.init_resource::<Time<Real>>();
        let mut sync = LocalTimelineSync::default();
        sync.set_synced(true);
        app.insert_resource(sync);
        app
    }

    fn link(app: &mut App, id: PeerId, state: P2P) -> Entity {
        let entity = app
            .world_mut()
            .spawn((state, LocalId(peer(0)), RemoteId(id), Connected))
            .id();
        app.world_mut()
            .resource_mut::<NetworkingMetadata>()
            .peer_map
            .insert(id, entity);
        entity
    }

    fn start_host(app: &mut App) {
        let mut session = app.world_mut().resource_mut::<P2PSession>();
        session.begin(SmallVec::from_slice(&[peer(1)]), None, Duration::ZERO);
        session.state = crate::P2PSessionState::Started {
            start_tick: Tick(20),
        };
        session.observe_local_id(peer(0));
    }

    #[test]
    fn application_can_accept_in_the_request_trigger_flush() {
        let mut app = app();
        start_host(&mut app);
        let newcomer = link(&mut app, peer(2), P2P::Inactive);
        app.add_observer(receive_request);
        app.add_observer(answer_request);
        app.add_observer(|request: On<P2PJoinRequested>, mut commands: Commands| {
            commands.trigger(P2PJoinAdmission {
                peer_id: request.peer_id,
                result: Ok(()),
            });
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(2),
            trigger: JoinRequest { request: 7 },
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Candidate));
        let session = app.world().resource::<P2PSession>();
        assert_eq!(session.started_peers().as_slice(), &[peer(1)]);
        assert_eq!(session.start_tick(), Some(Tick(20)));
    }

    #[test]
    fn refused_and_stale_application_answers_do_not_admit_a_link() {
        let mut app = app();
        start_host(&mut app);
        let newcomer = link(&mut app, peer(2), P2P::Inactive);
        app.add_observer(receive_request);
        app.add_observer(answer_request);
        app.world_mut().trigger(RemoteEvent {
            from: peer(2),
            trigger: JoinRequest { request: 7 },
        });
        // An answer for a peer with no pending admission is ignored.
        app.world_mut().trigger(P2PJoinAdmission {
            peer_id: peer(3),
            result: Ok(()),
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
        app.world_mut().trigger(P2PJoinAdmission {
            peer_id: peer(2),
            result: Err(JoinRejectReason::Refused),
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
        assert!(app.world().resource::<P2PSession>().is_started());
        // An answer after the decision was already sent is ignored.
        app.world_mut().trigger(P2PJoinAdmission {
            peer_id: peer(2),
            result: Ok(()),
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
    }

    #[derive(Resource, Default)]
    struct CaughtUp(Vec<P2PJoinCatchUp>);

    #[test]
    fn reordered_roster_readiness_still_starts_catch_up_once() {
        let mut app = app();
        link(&mut app, peer(0), P2P::Candidate);
        let id = JoinId {
            peer: peer(2),
            request: 7,
        };
        app.world_mut().resource_mut::<P2PSession>().join.attempt = Some(JoinAttempt {
            id,
            remote_peer: peer(0),
            hosts: SmallVec::new(),
            session_start_tick: None,
            stage: JoinStage::Requested,
            started_at: Duration::ZERO,
            roster_ready_epoch: None,
            acks: SmallVec::new(),
        });
        app.init_resource::<CaughtUp>();
        app.add_observer(receive_response);
        app.add_observer(receive_roster_ready);
        app.add_observer(|event: On<P2PJoinCatchUp>, mut caught: ResMut<CaughtUp>| {
            caught.0.push(*event.event())
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(0),
            trigger: JoinRosterReady { id, epoch: 5 },
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(0),
            trigger: JoinResponse {
                request: 7,
                result: Ok(JoinRoster {
                    epoch: 5,
                    peers: SmallVec::from_slice(&[peer(0), peer(1)]),
                    start_tick: Tick(20),
                }),
            },
        });
        app.world_mut().run_system_once(drive_join).unwrap();
        assert!(app.world().resource::<CaughtUp>().0.is_empty());
        link(&mut app, peer(1), P2P::Inactive);
        app.world_mut().run_system_once(drive_join).unwrap();
        app.world_mut().run_system_once(drive_join).unwrap();
        let events = &app.world().resource::<CaughtUp>().0;
        assert_eq!(
            events.as_slice(),
            &[P2PJoinCatchUp {
                epoch: 5,
                session_start_tick: Some(Tick(20)),
                remote_peer: peer(0),
            }]
        );
    }

    #[derive(Resource, Default)]
    struct Joined(Vec<P2PJoined>);

    #[test]
    fn activation_requires_the_reserved_proposal_and_updates_topology_before_observers() {
        let mut app = app();
        start_host(&mut app);
        let newcomer = link(&mut app, peer(2), P2P::Candidate);
        let incumbent = link(&mut app, peer(1), P2P::Joined);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(40);
        let id = JoinId {
            peer: peer(2),
            request: 7,
        };
        app.world_mut().resource_mut::<P2PSession>().join.admission = Some(HostAdmission {
            id,
            coordinator: peer(0),
            epoch: 0,
            stage: AdmissionStage::Ready,
            started_at: Duration::ZERO,
            acks: SmallVec::new(),
            ready_sent: true,
        });
        app.init_resource::<Joined>();
        app.add_observer(receive_proposal);
        app.add_observer(receive_activation);
        app.add_observer(
            |event: On<P2PJoined>,
             mut events: ResMut<Joined>,
             metadata: Res<NetworkingMetadata>,
             links: Query<&P2P>| {
                let link = event.link.unwrap();
                assert_eq!(links.get(link).unwrap(), &P2P::Joined);
                assert!(
                    metadata
                        .mode
                        .started_p2p_roster()
                        .unwrap()
                        .started
                        .contains(&link)
                );
                events.0.push(*event.event());
            },
        );
        let proposal = JoinActivationProposal {
            id,
            epoch: 0,
            tick: Tick(50),
        };
        // An unsolicited announcement, a foreign sender, and a different tick cannot activate.
        app.world_mut().trigger(RemoteEvent {
            from: peer(2),
            trigger: JoinActivate { proposal },
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(2),
            trigger: proposal,
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(1),
            trigger: JoinActivate { proposal },
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(2),
            trigger: JoinActivate {
                proposal: JoinActivationProposal {
                    tick: Tick(49),
                    ..proposal
                },
            },
        });
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(8);
        app.world_mut()
            .run_system_once(apply_join_activation)
            .unwrap();
        assert!(app.world().resource::<Joined>().0.is_empty());
        app.world_mut().trigger(RemoteEvent {
            from: peer(2),
            trigger: JoinActivate { proposal },
        });
        app.world_mut()
            .run_system_once(apply_join_activation)
            .unwrap();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Candidate));
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(1);
        app.world_mut()
            .run_system_once(apply_join_activation)
            .unwrap();
        app.world_mut()
            .run_system_once(apply_join_activation)
            .unwrap();
        assert_eq!(
            app.world().resource::<Joined>().0.as_slice(),
            &[P2PJoined {
                peer_id: peer(2),
                activate_tick: Tick(50),
                local_is_joiner: false,
                link: Some(newcomer),
            }]
        );
        assert_eq!(app.world().get::<P2P>(incumbent), Some(&P2P::Joined));
        let session = app.world().resource::<P2PSession>();
        assert_eq!(session.epoch(), 1);
        assert_eq!(session.start_tick(), Some(Tick(20)));
    }

    #[test]
    fn cancellation_handles_reordered_admission_and_unknown_newcomer_epoch() {
        let mut app = app();
        start_host(&mut app);
        app.world_mut().resource_mut::<P2PSession>().join.epoch = 5;
        link(&mut app, peer(1), P2P::Joined);
        let newcomer = link(&mut app, peer(2), P2P::Inactive);
        app.add_observer(receive_abort);
        app.add_observer(receive_admission);
        app.add_observer(abort_admission);
        let id = JoinId {
            peer: peer(2),
            request: 7,
        };
        app.world_mut().trigger(RemoteEvent {
            from: peer(1),
            trigger: JoinAbort {
                id,
                epoch: 5,
                reason: JoinRejectReason::Timeout,
            },
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(1),
            trigger: JoinAdmitted { id, epoch: 5 },
        });
        app.world_mut().run_system_once(drive_join).unwrap();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
        app.world_mut().trigger(RemoteEvent {
            from: peer(1),
            trigger: JoinAdmitted {
                id: JoinId { request: 8, ..id },
                epoch: 5,
            },
        });
        app.world_mut().run_system_once(drive_join).unwrap();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Candidate));
        // The newcomer can cancel before its response reveals epoch 5.
        app.world_mut().trigger(RemoteEvent {
            from: peer(2),
            trigger: JoinAbort {
                id: JoinId { request: 8, ..id },
                epoch: 0,
                reason: JoinRejectReason::Timeout,
            },
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
        assert!(app.world().resource::<P2PSession>().is_started());
    }
}
