//! Joining a running deterministic session.
//!
//! The newcomer asks one bootstrap peer. Its application answers [`P2PJoinRequested`] by
//! triggering [`P2PJoinAdmission`]; the request is not sent to the other applications. An accepted
//! admission is announced to the existing roster so its Links can stream inputs to the newcomer.
//! Once those Links are ready, the newcomer catches up through [`P2PJoinCatchUp`].
//!
//! After [`P2PJoinCatchUpComplete`], it proposes one future activation tick to every incumbent.
//! All must acknowledge the same proposal before it is announced. Membership and player creation
//! change together at that tick through [`P2PJoined`]. There is no separate membership/commit tick.
//!
//! This is a coordinated session protocol, not fault-tolerant consensus: an activation announcement
//! must reach every participant before its tick. A connection failure after announcement requires
//! session-level recovery, not unilateral cancellation of an already agreed activation.

use bevy_app::{App, FixedFirst, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_connection::p2p::{P2P, P2PRoster, P2PSessionPhase};
use lightyear_core::id::{LocalId, PeerId};
use lightyear_core::prelude::{LocalTimeline, Tick, TimelineSystems};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppTriggerExt, EventSender, RemoteEvent};
use lightyear_sync::prelude::SyncedLocalTimeline;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{P2PChannel, P2PSession};

/// Ask one existing player to admit this application to its running session.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoin {
    /// Discovery is application-owned; a declared Link must already identify this peer.
    pub bootstrap: PeerId,
}

/// Admission succeeded. Connect the listed peers; their Links are needed before catch-up.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct P2PJoinAccepted {
    /// Existing players, excluding the newcomer.
    pub roster: SmallVec<[PeerId; 4]>,
}

/// Application admission decision requested on the bootstrap peer only.
///
/// Reply by triggering [`P2PJoinAdmission`] with these identifiers. There is no implicit acceptance
/// or shared mutable decision resource. An observer can answer immediately in the same command
/// flush; an asynchronous lobby decision may answer later, within the negotiation timeout.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinRequested {
    pub peer_id: PeerId,
    pub request_id: u32,
    pub epoch: u32,
}

/// The application's answer to one [`P2PJoinRequested`]. Stale answers are ignored.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinAdmission {
    pub peer_id: PeerId,
    pub request_id: u32,
    pub epoch: u32,
    pub result: Result<(), JoinRejectReason>,
}

/// Why admission, catch-up, or activation negotiation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinRejectReason {
    NotStarted,
    Busy,
    Refused,
    Timeout,
    StaleEpoch,
    TooLate,
    CatchUpFailed,
    RosterIncomplete,
}

/// The newcomer's first gameplay tick. Create its player now, on every peer.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoined {
    pub peer_id: PeerId,
    pub activate_tick: Tick,
    pub local_is_joiner: bool,
    /// Input-receiving Link on incumbents; `None` on the newcomer itself.
    pub link: Option<Entity>,
}

/// All incumbent input Links are ready. Supply the joining application's world state.
///
/// Input-only applications reconstruct the initial world and replay its input archive. Snapshot
/// applications restore a snapshot instead. Catch-up owns its own progress budget and ends by
/// triggering [`P2PJoinCatchUpComplete`] or [`P2PJoinCancel`].
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinCatchUp {
    pub epoch: u32,
    pub session_start_tick: Option<Tick>,
    /// Admission coordinator, also the natural source for the catch-up state.
    pub bootstrap: PeerId,
}

/// Catch-up finished locally; negotiate a single future activation tick.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinCatchUpComplete {
    /// Last tick simulated with complete remote input coverage.
    pub caught_up_tick: Tick,
    /// Oldest restorable state, used as this newcomer's input rollback floor.
    pub history_start_tick: Tick,
}

/// Cancel a local join before activation has been announced.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinCancel {
    pub reason: JoinRejectReason,
}

/// Local join failed; the application may return to its lobby or try again.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PJoinRejected {
    pub reason: JoinRejectReason,
}

/// Join-specific session bookkeeping. A single instance is owned by [`P2PSession`].
#[derive(Debug, Default, Clone)]
pub struct P2PJoinState {
    pub(crate) epoch: u32,
    next_request: u32,
    /// Last cancelled attempt per remote peer; unordered control traffic may arrive after it.
    aborted: SmallVec<[(u32, JoinId); 4]>,
    pub(crate) attempt: Option<JoinAttempt>,
    pub(crate) admission: Option<HostAdmission>,
    pub(crate) activation: Option<PendingActivation>,
}

impl P2PJoinState {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinStage {
    Requested,
    Connecting,
    CatchingUp,
    Ready(Tick),
    Proposed(Tick),
    Announced,
}

#[derive(Debug, Clone)]
pub(crate) struct JoinAttempt {
    id: JoinId,
    bootstrap: PeerId,
    pub(crate) hosts: SmallVec<[PeerId; 4]>,
    pub(crate) session_start_tick: Option<Tick>,
    stage: JoinStage,
    started_at: Duration,
    roster_ready_epoch: Option<u32>,
    acks: SmallVec<[PeerId; 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionStage {
    AwaitingDecision,
    Connecting,
    Ready,
    Proposed(Tick),
}

#[derive(Debug, Clone)]
pub(crate) struct HostAdmission {
    id: JoinId,
    coordinator: PeerId,
    epoch: u32,
    stage: AdmissionStage,
    started_at: Duration,
    /// Only the bootstrap collects these; other hosts acknowledge their own Link once.
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

/// Notification of the bootstrap's accepted decision, not another application request.
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

pub struct P2PJoinPlugin;

impl Plugin for P2PJoinPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<P2PJoinProtocolPlugin>() {
            app.add_plugins(P2PJoinProtocolPlugin);
        }
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
        app.add_systems(PreUpdate, drive_join.after(MessageSystems::Receive));
        app.add_systems(
            FixedFirst,
            apply_join_activation.before(TimelineSystems::IncrementLocal),
        );
    }
}

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

fn begin_join(
    trigger: On<P2PJoin>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
    metadata: Res<NetworkingMetadata>,
    links: Query<(&P2P, &LocalId)>,
    mut senders: Query<&mut EventSender<JoinRequest>>,
) {
    if session.is_started()
        || session.is_joining()
        || !matches!(session.state(), crate::P2PSessionState::Stopped)
    {
        return;
    }
    let Some(&link) = metadata.peer_map.get(&trigger.bootstrap) else {
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
        trigger.bootstrap,
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
        bootstrap: trigger.bootstrap,
        hosts: SmallVec::new(),
        session_start_tick: None,
        stage: JoinStage::Requested,
        started_at: time.elapsed(),
        roster_ready_epoch: None,
        acks: SmallVec::new(),
    });
    commands.entity(link).insert(P2P::Candidate);
    tracing::info!(bootstrap = ?trigger.bootstrap, request, "asking to join a running P2P session");
}

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
    if session.join.was_aborted(JoinId {
        peer: trigger.from,
        request: trigger.trigger.request,
    }) {
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
    let id = JoinId {
        peer: trigger.from,
        request: trigger.trigger.request,
    };
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
    commands.trigger(P2PJoinRequested {
        peer_id: id.peer,
        request_id: id.request,
        epoch,
    });
}

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
    if admission.id
        != (JoinId {
            peer: trigger.peer_id,
            request: trigger.request_id,
        })
        || admission.epoch != trigger.epoch
        || admission.stage != AdmissionStage::AwaitingDecision
    {
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
    peers.sort_unstable_by_key(|peer| peer.to_bits());
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

fn receive_response(
    trigger: On<RemoteEvent<JoinResponse>>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
) {
    let Some(attempt) = session.join.attempt.as_ref() else {
        return;
    };
    if trigger.from != attempt.bootstrap
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
    if roster.peers.is_empty()
        || !roster.peers.contains(&attempt.bootstrap)
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
    commands.trigger(P2PJoinAccepted {
        roster: roster.peers.clone(),
    });
}

fn receive_admission(
    trigger: On<RemoteEvent<JoinAdmitted>>,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
    metadata: Res<NetworkingMetadata>,
    mut senders: Query<&mut EventSender<JoinLinkReady>>,
) {
    let message = trigger.trigger;
    if !session.is_started() || !session.started_peers().contains(&trigger.from) {
        return;
    }
    if session.join.was_aborted(message.id) {
        return;
    }
    let reason = if message.epoch != session.epoch() {
        Some(JoinRejectReason::StaleEpoch)
    } else if session.join.admission.is_some()
        || session.join.activation.is_some()
        || session.started_peers().contains(&message.id.peer)
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

fn receive_roster_ready(
    trigger: On<RemoteEvent<JoinRosterReady>>,
    mut session: ResMut<P2PSession>,
) {
    let Some(attempt) = session.join.attempt.as_mut() else {
        return;
    };
    if trigger.from == attempt.bootstrap && trigger.trigger.id == attempt.id {
        attempt.roster_ready_epoch = Some(trigger.trigger.epoch);
    }
}

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
    // Before the response arrives, only the bootstrap is known.
    for peer in core::iter::once(attempt.bootstrap).chain(
        attempt
            .hosts
            .into_iter()
            .filter(|p| *p != attempt.bootstrap),
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
        && (trigger.from == attempt.bootstrap || attempt.hosts.contains(&trigger.from))
    {
        commands.trigger(P2PJoinCancel {
            reason: message.reason,
        });
    } else if let Some(admission) = session.join.admission.as_ref()
        && message.id == admission.id
        && message.epoch == admission.epoch
        && (trigger.from == admission.id.peer || trigger.from == admission.coordinator)
    {
        commands.trigger(AbortAdmission {
            reason: message.reason,
        });
    }
}

fn drive_join(
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    time: Res<Time<Real>>,
    timeline: Res<LocalTimeline>,
    synced: Option<SyncedLocalTimeline>,
    metadata: Res<NetworkingMetadata>,
    links: Query<(&P2P, Has<Connected>)>,
    mut link_ready: Query<&mut EventSender<JoinLinkReady>>,
    mut roster_ready: Query<&mut EventSender<JoinRosterReady>>,
    mut proposals: Query<&mut EventSender<JoinActivationProposal>>,
    mut activations: Query<&mut EventSender<JoinActivate>>,
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
            && let Some(&link) = metadata.peer_map.get(&admission.id.peer)
            && let Ok((state, connected)) = links.get(link)
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
                    &metadata,
                    &mut link_ready,
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
                &metadata,
                &mut roster_ready,
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
                let Some(&link) = metadata.peer_map.get(peer) else {
                    all_connected = false;
                    continue;
                };
                let Ok((state, connected)) = links.get(link) else {
                    all_connected = false;
                    continue;
                };
                if *state == P2P::Inactive {
                    commands.entity(link).insert(P2P::Candidate);
                }
                all_connected &= connected;
            }
            if all_connected && attempt.roster_ready_epoch == Some(epoch) && synced.is_some() {
                attempt.stage = JoinStage::CatchingUp;
                commands.trigger(P2PJoinCatchUp {
                    epoch,
                    session_start_tick: attempt.session_start_tick,
                    bootstrap: attempt.bootstrap,
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
                trigger_peer(peer, proposal, &metadata, &mut proposals);
            }
            attempt.acks.clear();
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
                trigger_peer(peer, JoinActivate { proposal }, &metadata, &mut activations);
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

fn apply_join_activation(
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    timeline: Res<LocalTimeline>,
    metadata: Res<NetworkingMetadata>,
    mut phase: ResMut<P2PSessionPhase>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use bevy_ecs::system::RunSystemOnce;
    use lightyear_core::id::RemoteId;
    use lightyear_sync::prelude::LocalTimelineSync;

    fn peer(id: u64) -> PeerId {
        PeerId::Local(id)
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
                request_id: request.request_id,
                epoch: request.epoch,
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
        app.world_mut().trigger(P2PJoinAdmission {
            peer_id: peer(2),
            request_id: 6,
            epoch: 0,
            result: Ok(()),
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
        app.world_mut().trigger(P2PJoinAdmission {
            peer_id: peer(2),
            request_id: 7,
            epoch: 0,
            result: Err(JoinRejectReason::Refused),
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
        assert!(app.world().resource::<P2PSession>().is_started());
        assert!(
            app.world()
                .resource::<P2PSession>()
                .join
                .admission
                .is_none()
        );
    }

    #[derive(Resource, Default)]
    struct CaughtUp(Vec<P2PJoinCatchUp>);

    #[test]
    fn reordered_roster_readiness_still_starts_catch_up_once() {
        let mut app = app();
        link(&mut app, peer(0), P2P::Candidate);
        link(&mut app, peer(1), P2P::Candidate);
        let id = JoinId {
            peer: peer(2),
            request: 7,
        };
        app.world_mut().resource_mut::<P2PSession>().join.attempt = Some(JoinAttempt {
            id,
            bootstrap: peer(0),
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
        app.world_mut().run_system_once(drive_join).unwrap();
        let events = &app.world().resource::<CaughtUp>().0;
        assert_eq!(
            events.as_slice(),
            &[P2PJoinCatchUp {
                epoch: 5,
                session_start_tick: Some(Tick(20)),
                bootstrap: peer(0),
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
    fn an_abort_before_admission_cannot_block_the_next_join() {
        let mut app = app();
        start_host(&mut app);
        link(&mut app, peer(1), P2P::Joined);
        let newcomer = link(&mut app, peer(2), P2P::Inactive);
        app.add_observer(receive_abort);
        app.add_observer(receive_admission);
        let id = JoinId {
            peer: peer(2),
            request: 7,
        };
        app.world_mut().trigger(RemoteEvent {
            from: peer(1),
            trigger: JoinAbort {
                id,
                epoch: 0,
                reason: JoinRejectReason::Timeout,
            },
        });
        app.world_mut().trigger(RemoteEvent {
            from: peer(1),
            trigger: JoinAdmitted { id, epoch: 0 },
        });
        app.world_mut().run_system_once(drive_join).unwrap();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Inactive));
        app.world_mut().trigger(RemoteEvent {
            from: peer(1),
            trigger: JoinAdmitted {
                id: JoinId { request: 8, ..id },
                epoch: 0,
            },
        });
        app.world_mut().run_system_once(drive_join).unwrap();
        assert_eq!(app.world().get::<P2P>(newcomer), Some(&P2P::Candidate));
        assert!(app.world().resource::<P2PSession>().is_started());
    }
}
