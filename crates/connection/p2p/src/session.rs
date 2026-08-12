use alloc::vec::Vec;
use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use core::hash::Hasher;
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_topology::NetworkTopologySystems;
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_link::prelude::{Unlink, UnlinkReason};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageReceiver, MessageSender};
use lightyear_serde::ToBytes;
use lightyear_sync::prelude::SyncedLocalTimeline;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

const DEFAULT_START_DELAY_TICKS: u16 = 120;
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Public lifecycle of the deterministic P2P session.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum P2PSessionState {
    /// No deterministic session is active. P2P Links may still be connected.
    #[default]
    Stopped,
    /// The Links captured by [`P2PStart`] are becoming ready and agreeing on a start tick.
    Starting {
        /// The agreed tick, once every peer has advertised its earliest start tick.
        start_tick: Option<Tick>,
    },
    /// The deterministic session has started.
    Started {
        /// Tick at which [`P2PStarted`] was triggered on every peer.
        start_tick: Tick,
    },
}

/// Stable identity and start-negotiation progress for one remote peer in the frozen cohort.
#[derive(Debug, Clone, Copy)]
struct RemotePeerStart {
    /// Stable identity used to find the peer's current P2P Link.
    peer_id: PeerId,
    /// Earliest start tick advertised by this remote peer.
    ready_tick: Option<Tick>,
    /// Start tick this remote peer acknowledged after seeing every Ready message.
    acknowledged_tick: Option<Tick>,
}

impl RemotePeerStart {
    fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            ready_tick: None,
            acknowledged_tick: None,
        }
    }
}

/// Application-global bookkeeping and state for one deterministic P2P session.
///
/// [`P2PSessionPlugin`] initializes this resource. Applications normally only declare [`P2P`]
/// Links and trigger [`P2PStart`]; they do not need to create or update the resource themselves.
#[derive(Resource, Debug)]
pub struct P2PSession {
    /// Lead added to the local tick when this peer becomes ready.
    start_delay_ticks: u16,
    /// Maximum wall-clock time allowed for one start attempt.
    start_timeout: Duration,
    /// Wall-clock timestamp at which the current start attempt began.
    start_started_at: Option<Duration>,
    /// Current deterministic-session lifecycle.
    state: P2PSessionState,
    /// Local start-attempt number carried by messages to reject packets from older attempts.
    ///
    /// Peers must begin start attempts in the same sequence for their generations to match.
    /// Wrapping is harmless unless delayed messages survive `u32::MAX` complete attempts.
    generation: u32,
    /// Earliest start tick advertised by this app after its Links and input timeline are ready.
    local_ready_tick: Option<Tick>,
    /// Whether this app has queued its Ready message on every frozen remote Link.
    local_ready_sent: bool,
    /// Whether this app has queued its StartAcknowledgement on every frozen remote Link.
    local_acknowledgement_sent: bool,
    /// Hash of the complete roster frozen by [`P2PStart`].
    roster_hash: Option<u64>,
    /// Transient negotiation progress for remote peers frozen by [`P2PStart`].
    ///
    /// This stores stable peer IDs rather than Link entities and is cleared after the barrier.
    /// The local peer is not included; its progress is stored in the `local_*` fields above.
    remote_peers: SmallVec<[RemotePeerStart; 4]>,
    /// Set when a peer advertises a different roster for the same attempt.
    roster_mismatch: bool,
}

impl Default for P2PSession {
    fn default() -> Self {
        Self {
            start_delay_ticks: DEFAULT_START_DELAY_TICKS,
            start_timeout: DEFAULT_START_TIMEOUT,
            start_started_at: None,
            state: P2PSessionState::Stopped,
            generation: 0,
            local_ready_tick: None,
            local_ready_sent: false,
            local_acknowledgement_sent: false,
            roster_hash: None,
            remote_peers: SmallVec::new(),
            roster_mismatch: false,
        }
    }
}

impl P2PSession {
    /// Set the lead time used when proposing a common future start tick.
    pub fn with_start_delay_ticks(mut self, ticks: u16) -> Self {
        self.start_delay_ticks = ticks;
        self
    }

    /// Set the maximum wall-clock duration allowed for a start barrier.
    pub fn with_start_timeout(mut self, timeout: Duration) -> Self {
        self.start_timeout = timeout;
        self
    }

    /// Current deterministic session lifecycle.
    pub fn state(&self) -> P2PSessionState {
        self.state
    }

    /// Tick at which the current session starts or started, once it has been agreed.
    pub fn start_tick(&self) -> Option<Tick> {
        match self.state {
            P2PSessionState::Starting { start_tick } => start_tick,
            P2PSessionState::Started { start_tick } => Some(start_tick),
            P2PSessionState::Stopped => None,
        }
    }

    /// Whether the deterministic session has started.
    pub fn is_started(&self) -> bool {
        matches!(self.state, P2PSessionState::Started { .. })
    }

    /// Reset negotiation state and freeze the supplied remote identities into a new start attempt.
    fn begin(
        &mut self,
        remote_peer_ids: SmallVec<[PeerId; 4]>,
        roster_hash: u64,
        started_at: Duration,
    ) {
        self.generation = self.generation.wrapping_add(1);
        self.start_started_at = Some(started_at);
        self.local_ready_tick = None;
        self.local_ready_sent = false;
        self.local_acknowledgement_sent = false;
        self.roster_hash = Some(roster_hash);
        self.remote_peers = remote_peer_ids
            .into_iter()
            .map(RemotePeerStart::new)
            .collect();
        self.roster_mismatch = false;
        self.state = P2PSessionState::Starting { start_tick: None };
    }

    /// Clear the active cohort and return to the lobby/stopped state.
    fn stop(&mut self) {
        self.state = P2PSessionState::Stopped;
        self.clear_barrier_progress();
    }

    /// Drop transient barrier bookkeeping while preserving the public Started state.
    fn clear_barrier_progress(&mut self) {
        self.start_started_at = None;
        self.local_ready_tick = None;
        self.local_ready_sent = false;
        self.local_acknowledgement_sent = false;
        self.roster_hash = None;
        self.remote_peers.clear();
        self.roster_mismatch = false;
    }

    fn contains_remote(&self, peer_id: PeerId) -> bool {
        self.remote_peers.iter().any(|peer| peer.peer_id == peer_id)
    }

    fn timed_out_at(&self, now: Duration) -> bool {
        self.start_started_at
            .is_some_and(|started_at| now.saturating_sub(started_at) >= self.start_timeout)
    }

    /// Record one message received from a remote Link in the current start attempt.
    ///
    /// Delayed messages from older generations and messages from Links outside the frozen cohort
    /// are ignored. Repeated identical messages are harmless; conflicting repeats are logged and
    /// the first value remains authoritative.
    fn receive(&mut self, peer_id: PeerId, message: P2PSessionMessage) {
        // Reliable packets from a previous start can arrive after a stop/restart cycle.
        let generation = match message {
            P2PSessionMessage::Ready { generation, .. }
            | P2PSessionMessage::StartAcknowledgement { generation, .. } => generation,
        };
        if generation != self.generation {
            return;
        }
        let Some(peer) = self
            .remote_peers
            .iter_mut()
            .find(|peer| peer.peer_id == peer_id)
        else {
            return;
        };
        let message_roster_hash = match message {
            P2PSessionMessage::Ready { roster_hash, .. }
            | P2PSessionMessage::StartAcknowledgement { roster_hash, .. } => roster_hash,
        };
        if self.roster_hash != Some(message_roster_hash) {
            self.roster_mismatch = true;
            tracing::warn!(
                ?peer_id,
                local_roster_hash = ?self.roster_hash,
                remote_roster_hash = message_roster_hash,
                "peer advertised a different P2P roster"
            );
            return;
        }
        match message {
            P2PSessionMessage::Ready {
                earliest_start_tick,
                ..
            } => match peer.ready_tick {
                None => peer.ready_tick = Some(earliest_start_tick),
                Some(previous) if previous != earliest_start_tick => tracing::warn!(
                    ?peer_id,
                    ?previous,
                    ?earliest_start_tick,
                    "ignoring conflicting P2P ready message"
                ),
                Some(_) => {}
            },
            P2PSessionMessage::StartAcknowledgement { start_tick, .. } => {
                match peer.acknowledged_tick {
                    None => peer.acknowledged_tick = Some(start_tick),
                    Some(previous) if previous != start_tick => tracing::warn!(
                        ?peer_id,
                        ?previous,
                        ?start_tick,
                        "ignoring conflicting P2P start acknowledgement"
                    ),
                    Some(_) => {}
                }
            }
        }
    }

    /// Advance the local start negotiation from readiness through the agreed start tick.
    ///
    /// Each peer proposes `current tick + start delay`. Once all proposals exist, their maximum
    /// is the common start tick. Every peer must then acknowledge that same value. The transition
    /// to Started happens one tick early in `PreUpdate`, so [`P2PStarted`] observers can create the
    /// deterministic world before `FixedFirst` advances to the agreed tick.
    fn advance(&mut self, tick: Tick, timeline_synced: bool) -> AdvanceResult {
        if !matches!(self.state, P2PSessionState::Starting { .. }) {
            return AdvanceResult::Waiting;
        }
        if self.roster_mismatch {
            return AdvanceResult::Failed;
        }

        // Do not advertise readiness until the shared input timeline is usable.
        if timeline_synced && self.local_ready_tick.is_none() {
            self.local_ready_tick = Some(tick + i32::from(self.start_delay_ticks));
        }
        let Some(local_ready_tick) = self.local_ready_tick else {
            return AdvanceResult::Waiting;
        };
        let Some(start_tick) = self
            .remote_peers
            .iter()
            .try_fold(local_ready_tick, |latest, peer| {
                peer.ready_tick.map(|ready| latest.max(ready))
            })
        else {
            return AdvanceResult::Waiting;
        };

        // The maximum proposal is deterministic on every peer, regardless of arrival order.
        self.state = P2PSessionState::Starting {
            start_tick: Some(start_tick),
        };

        if self
            .remote_peers
            .iter()
            .any(|peer| peer.acknowledged_tick.is_none())
        {
            return AdvanceResult::Waiting;
        }
        if let Some((peer_id, acknowledged)) = self.remote_peers.iter().find_map(|peer| {
            peer.acknowledged_tick
                .filter(|acknowledged| *acknowledged != start_tick)
                .map(|acknowledged| (peer.peer_id, acknowledged))
        }) {
            tracing::warn!(
                ?peer_id,
                ?start_tick,
                ?acknowledged,
                "peer acknowledged a different P2P start tick"
            );
            return AdvanceResult::Failed;
        }

        // Receiving the final acknowledgement after the target tick is too late to create the
        // shared world deterministically before that tick.
        if tick >= start_tick {
            tracing::warn!(
                ?tick,
                ?start_tick,
                "P2P start acknowledgement arrived after the agreed tick"
            );
            return AdvanceResult::Failed;
        }
        if tick + 1 < start_tick {
            return AdvanceResult::Waiting;
        }
        self.state = P2PSessionState::Started { start_tick };
        AdvanceResult::Started(start_tick)
    }

    /// Return each locally available Ready or acknowledgement once.
    ///
    /// The caller broadcasts each returned message to every current candidate Link. The transport
    /// channel provides retransmission; these flags only prevent enqueuing duplicates every frame.
    fn collect_outbound(&mut self, outbound: &mut SmallVec<[P2PSessionMessage; 2]>) {
        let Some(roster_hash) = self.roster_hash else {
            return;
        };
        if let Some(earliest_start_tick) = self.local_ready_tick
            && !self.local_ready_sent
        {
            self.local_ready_sent = true;
            outbound.push(P2PSessionMessage::Ready {
                generation: self.generation,
                roster_hash,
                earliest_start_tick,
            });
        }
        if let Some(start_tick) = self.start_tick()
            && !self.local_acknowledgement_sent
        {
            self.local_acknowledgement_sent = true;
            outbound.push(P2PSessionMessage::StartAcknowledgement {
                generation: self.generation,
                roster_hash,
                start_tick,
            });
        }
    }
}

/// Trigger this locally on every peer after declaring the P2P Links that should form the initial
/// session cohort.
///
/// The Links don't have to be connected yet when `P2PStart` is called. Lightyear waits for every
/// captured Link and the synchronized input timeline, negotiates a common future tick, then
/// triggers [`P2PStarted`]. Every Link must expose the same stable [`LocalId`] and a distinct
/// [`RemoteId`]. If the barrier does not complete within the configured timeout (5 seconds by
/// default), its candidates return to [`P2P::Inactive`].
#[derive(Event, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct P2PStart;

/// Trigger this locally to leave the deterministic session.
///
/// This does not send a stop request over the network. Deterministic games can schedule it at the
/// same tick on every peer; lobby-driven applications can coordinate it separately. Set
/// [`unlink`](Self::unlink) to also terminate every currently declared [`P2P`] Link.
#[derive(Event, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct P2PStop {
    /// Whether to unlink every currently declared P2P Link after stopping the session.
    pub unlink: bool,
}

/// Triggered immediately before the fixed simulation advances to the agreed tick.
///
/// Applications can observe this to create the shared deterministic world in the same order on
/// every peer. Prediction treats the preceding tick as the initial rollback snapshot so a late
/// input for the first gameplay tick can still be corrected.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PStarted {
    /// Common first gameplay tick. The preceding tick is the initial rollback boundary.
    pub start_tick: Tick,
}

/// Triggered after [`P2PStop`] returns the deterministic session to its stopped state.
///
/// Link connection state is affected only when [`P2PStop::unlink`] is `true`.
#[derive(Event, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct P2PStopped;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum P2PSessionMessage {
    /// Advertise that one peer is locally ready and its earliest safe start tick.
    Ready {
        /// Start-attempt number used to discard delayed messages from earlier attempts.
        generation: u32,
        /// Hash of the complete roster, including the sender.
        roster_hash: u64,
        /// Sender's current tick plus its configured start delay.
        earliest_start_tick: Tick,
    },
    /// Confirm the maximum Ready tick selected as the common start tick.
    StartAcknowledgement {
        /// Start-attempt number used to discard delayed messages from earlier attempts.
        generation: u32,
        /// Hash of the complete roster, including the sender.
        roster_hash: u64,
        /// Common start tick calculated by the sender.
        start_tick: Tick,
    },
}

/// Private reliable delivery queue for P2P lifecycle control messages.
struct P2PSessionChannel;

/// Registers the private P2P control protocol.
///
/// This plugin is installed by Lightyear's shared plugin setup so P2P-enabled conventional
/// clients and servers reserve identical channel and message IDs. [`P2PSessionPlugin`] installs
/// it as a fallback for applications that use this crate directly.
#[doc(hidden)]
pub struct P2PProtocolPlugin;

impl Plugin for P2PProtocolPlugin {
    fn build(&self, app: &mut App) {
        // No generic reliable control channel exists at this layer. Replication's reliable
        // channels are optional and must not become a dependency of the P2P session. The two
        // handshake messages tolerate reordering, so a private unordered-reliable channel is
        // sufficient and avoids coupling their delivery queue to gameplay traffic.
        app.add_channel::<P2PSessionChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<P2PSessionMessage>()
            .add_direction(NetworkDirection::Bidirectional);
    }
}

/// Installs the P2P session lifecycle and start-tick negotiation.
pub struct P2PSessionPlugin;

impl Plugin for P2PSessionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<P2PProtocolPlugin>() {
            app.add_plugins(P2PProtocolPlugin);
        }
        app.init_resource::<P2PSession>();

        app.add_observer(start_session);
        app.add_observer(stop_session);
        app.add_systems(
            PreUpdate,
            drive_session
                .after(MessageSystems::Receive)
                // A successful barrier replaces Candidate with Joined. Refresh the cached
                // topology in the same PreUpdate so FixedMain sees the new membership at the
                // agreed start tick.
                .before(NetworkTopologySystems::Update),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvanceResult {
    Waiting,
    Started(Tick),
    Failed,
}

/// Hash a roster independently of Link entity allocation and local/remote ordering.
fn roster_hash(local_id: PeerId, remote_ids: &[PeerId]) -> u64 {
    let mut encoded_peers = Vec::with_capacity(remote_ids.len() + 1);
    for peer_id in core::iter::once(&local_id).chain(remote_ids) {
        let mut encoded = Vec::with_capacity(peer_id.bytes_len());
        peer_id
            .to_bytes(&mut encoded)
            .expect("serializing a PeerId into memory cannot fail");
        encoded_peers.push(encoded);
    }
    encoded_peers.sort_unstable();

    let mut hasher = seahash::SeaHasher::new();
    for encoded in encoded_peers {
        hasher.write(&encoded);
    }
    hasher.finish()
}

/// Signal the start of a new deterministic P2P session.
///
/// Every currently declared P2P Link is used as a candidate for the new session.
/// Remove the P2P component from Links that should not participate before triggering this event.
///
/// Starting the session involves coordinating between each peer so that they can agree
/// on a common start tick. After the session is started, the P2P Links will be in the Joined state
/// and can be used for deterministic gameplay.
fn start_session(
    _trigger: On<P2PStart>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    real_time: Res<Time<Real>>,
    links: Query<(
        Entity,
        &P2P,
        Option<&LocalId>,
        Option<&RemoteId>,
        Has<MessageReceiver<P2PSessionMessage>>,
    )>,
) {
    if !matches!(session.state, P2PSessionState::Stopped) {
        tracing::error!(
            state = ?session.state,
            "rejecting P2PStart because a session is already starting or running"
        );
        return;
    }

    let candidates: SmallVec<[(Entity, Option<PeerId>, Option<PeerId>, bool); 4]> = links
        .iter()
        .filter_map(|(entity, state, local_id, remote_id, has_receiver)| {
            (*state == P2P::Inactive).then_some((
                entity,
                local_id.map(|id| id.0),
                remote_id.map(|id| id.0),
                has_receiver,
            ))
        })
        .collect();
    let peer_count = candidates.len().saturating_add(1);
    if candidates.is_empty() {
        tracing::warn!("P2PStart requires at least one inactive remote P2P Link");
        return;
    }
    let Some(local_id) = candidates[0].1 else {
        tracing::warn!("P2PStart requires every candidate Link to have a LocalId");
        return;
    };
    if candidates
        .iter()
        .any(|(_, candidate_local_id, _, _)| *candidate_local_id != Some(local_id))
    {
        tracing::warn!(
            ?local_id,
            "P2PStart requires every candidate Link to have the same LocalId"
        );
        return;
    }

    let mut remote_ids = SmallVec::<[PeerId; 4]>::new();
    for (_, _, remote_id, _) in &candidates {
        let Some(remote_id) = *remote_id else {
            tracing::warn!("P2PStart requires every candidate Link to have a RemoteId");
            return;
        };
        if remote_id == local_id {
            tracing::warn!(
                ?local_id,
                "a P2P Link cannot identify the local peer as remote"
            );
            return;
        }
        if remote_ids.contains(&remote_id) {
            tracing::warn!(
                ?remote_id,
                "P2PStart found duplicate remote peer identities"
            );
            return;
        }
        remote_ids.push(remote_id);
    }
    let roster_hash = roster_hash(local_id, &remote_ids);

    for (entity, _, _, has_receiver) in &candidates {
        commands.entity(*entity).insert(P2P::Candidate);
        if !has_receiver {
            commands
                .entity(*entity)
                .insert(MessageReceiver::<P2PSessionMessage>::default());
        }
    }

    tracing::info!(peer_count, roster_hash, "starting P2P session negotiation");
    session.begin(remote_ids, roster_hash, real_time.elapsed());
}

/// Stop the local deterministic session, optionally unlink every P2P Link, and emit
/// [`P2PStopped`].
///
/// This does not send a session-stop message to remote peers. An unlink request reaches the
/// concrete transport, which closes that Link and normally becomes visible to its remote endpoint
/// as a transport disconnection.
fn stop_session(
    trigger: On<P2PStop>,
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    links: Query<(Entity, &P2P)>,
) {
    let was_running = !matches!(session.state, P2PSessionState::Stopped);
    if was_running {
        session.stop();
    }

    for (entity, state) in &links {
        if *state != P2P::Inactive {
            commands.entity(entity).insert(P2P::Inactive);
        }
        if trigger.unlink {
            commands.trigger(Unlink {
                entity,
                reason: UnlinkReason::UserRequested(Some("P2P session stopped".into())),
            });
        }
    }

    if was_running {
        tracing::info!(unlink = trigger.unlink, "P2P session stopped");
        commands.trigger(P2PStopped);
    }
}

type P2PLinkQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static P2P,
        Option<&'static RemoteId>,
        Has<Connected>,
        Option<&'static mut MessageSender<P2PSessionMessage>>,
        Option<&'static mut MessageReceiver<P2PSessionMessage>>,
    ),
    With<P2P>,
>;

fn transition_candidates(commands: &mut Commands, links: &mut P2PLinkQuery, next_state: P2P) {
    for (entity, state, ..) in links {
        if *state == P2P::Candidate {
            commands.entity(entity).insert(next_state);
        }
    }
}

/// Drive the current start attempt once per frame before fixed simulation.
///
/// Incoming messages update per-remote-peer progress. Once every frozen Link is connected and the
/// input timeline is synchronized, [`P2PSession::advance`] chooses and acknowledges the start
/// tick. Newly available local messages are then queued once on every remote Link.
fn drive_session(
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    timeline: Res<LocalTimeline>,
    synced_timeline: Option<SyncedLocalTimeline>,
    real_time: Res<Time<Real>>,
    mut links: P2PLinkQuery,
) {
    if !matches!(session.state, P2PSessionState::Starting { .. }) {
        return;
    }

    let timed_out = session.timed_out_at(real_time.elapsed());
    let expected_peer_count = session.remote_peers.len();
    let mut found_peers = SmallVec::<[PeerId; 4]>::new();
    let mut cohort_intact = true;
    let mut all_connected = true;
    for (entity, state, remote_id, connected, sender, receiver) in &mut links {
        if *state != P2P::Candidate {
            continue;
        }
        let Some(remote_id) = remote_id.map(|remote_id| remote_id.0) else {
            cohort_intact = false;
            all_connected = false;
            continue;
        };
        if !session.contains_remote(remote_id) || found_peers.contains(&remote_id) {
            cohort_intact = false;
            all_connected = false;
            continue;
        }
        found_peers.push(remote_id);
        if !connected || sender.is_none() {
            all_connected = false;
        }
        // Receiver insertion requested by P2PStart can still be deferred for this frame. It does
        // not prevent us from sending our own Ready message once every Link is connected.
        let Some(mut receiver) = receiver else {
            continue;
        };
        for message in receiver.receive() {
            tracing::trace!(?entity, ?remote_id, ?message, tick = ?timeline.tick(), "received P2P session message");
            session.receive(remote_id, message);
        }
    }
    cohort_intact &= found_peers.len() == expected_peer_count;

    if !cohort_intact {
        session.stop();
        transition_candidates(&mut commands, &mut links, P2P::Inactive);
        tracing::warn!("P2P session start negotiation aborted because its cohort changed");
        return;
    }
    if session.roster_mismatch {
        session.stop();
        transition_candidates(&mut commands, &mut links, P2P::Inactive);
        tracing::warn!("P2P session start negotiation failed because peer rosters differ");
        return;
    }
    if timed_out {
        let timeout = session.start_timeout;
        session.stop();
        transition_candidates(&mut commands, &mut links, P2P::Inactive);
        tracing::warn!(?timeout, "P2P session start negotiation timed out");
        return;
    }
    if !all_connected {
        return;
    }
    match session.advance(timeline.tick(), synced_timeline.is_some()) {
        AdvanceResult::Started(start_tick) => {
            tracing::info!(?start_tick, "P2P session started");
            transition_candidates(&mut commands, &mut links, P2P::Joined);
            session.clear_barrier_progress();
            commands.trigger(P2PStarted { start_tick });
            return;
        }
        AdvanceResult::Failed => {
            tracing::warn!("P2P session start negotiation failed");
            session.stop();
            transition_candidates(&mut commands, &mut links, P2P::Inactive);
            return;
        }
        AdvanceResult::Waiting => {}
    }

    let mut outbound = SmallVec::<[P2PSessionMessage; 2]>::new();
    session.collect_outbound(&mut outbound);
    for (entity, state, remote_id, _, sender, _) in &mut links {
        if *state != P2P::Candidate
            || !remote_id.is_some_and(|remote_id| session.contains_remote(remote_id.0))
        {
            continue;
        }
        let Some(mut sender) = sender else {
            continue;
        };
        for &message in &outbound {
            tracing::trace!(?entity, ?message, tick = ?timeline.tick(), "sending P2P session message");
            sender.send::<P2PSessionChannel>(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(index: u64) -> PeerId {
        PeerId::Entity(index)
    }

    #[test]
    fn start_freezes_inactive_links_and_ignores_links_declared_later() {
        let mut app = App::new();
        app.init_resource::<P2PSession>();
        app.init_resource::<Time<Real>>();
        app.add_observer(start_session);
        let local_id = peer(0);
        let second = app
            .world_mut()
            .spawn((P2P::Inactive, LocalId(local_id), RemoteId(peer(2))))
            .id();
        let first = app
            .world_mut()
            .spawn((P2P::Inactive, LocalId(local_id), RemoteId(peer(1))))
            .id();

        app.world_mut().trigger(P2PStart);
        app.world_mut().flush();
        assert!(
            app.world()
                .entity(first)
                .contains::<MessageReceiver<P2PSessionMessage>>()
        );
        assert_eq!(
            app.world().entity(first).get::<P2P>(),
            Some(&P2P::Candidate)
        );
        assert_eq!(
            app.world().entity(second).get::<P2P>(),
            Some(&P2P::Candidate)
        );
        let generation = {
            let session = app.world().resource::<P2PSession>();
            assert_eq!(
                session.state(),
                P2PSessionState::Starting { start_tick: None }
            );
            assert_eq!(session.remote_peers.len(), 2);
            assert!(session.contains_remote(peer(1)));
            assert!(session.contains_remote(peer(2)));
            session.generation
        };

        app.world_mut().trigger(P2PStart);
        app.world_mut().flush();
        assert_eq!(
            app.world().resource::<P2PSession>().generation,
            generation,
            "a duplicate start must not restart or expand the active barrier"
        );

        let late = app
            .world_mut()
            .spawn((P2P::Inactive, LocalId(local_id), RemoteId(peer(3))))
            .id();
        assert_eq!(app.world().entity(late).get::<P2P>(), Some(&P2P::Inactive));
        assert!(
            !app.world()
                .resource::<P2PSession>()
                .contains_remote(peer(3))
        );
    }

    #[test]
    fn ready_peers_choose_and_acknowledge_the_latest_tick() {
        let mut session = P2PSession::default().with_start_delay_ticks(10);
        let local = peer(0);
        let first = peer(1);
        let second = peer(2);
        let remote_peers = SmallVec::from_slice(&[first, second]);
        let roster_hash = roster_hash(local, &remote_peers);
        session.begin(remote_peers, roster_hash, Duration::ZERO);

        assert_eq!(session.advance(Tick(5), false), AdvanceResult::Waiting);
        assert_eq!(session.advance(Tick(6), true), AdvanceResult::Waiting);
        session.receive(
            first,
            P2PSessionMessage::Ready {
                generation: session.generation,
                roster_hash,
                earliest_start_tick: Tick(18),
            },
        );
        session.receive(
            second,
            P2PSessionMessage::Ready {
                generation: session.generation,
                roster_hash,
                earliest_start_tick: Tick(17),
            },
        );
        assert_eq!(session.advance(Tick(7), true), AdvanceResult::Waiting);
        assert_eq!(session.start_tick(), Some(Tick(18)));

        for link in [first, second] {
            session.receive(
                link,
                P2PSessionMessage::StartAcknowledgement {
                    generation: session.generation,
                    roster_hash,
                    start_tick: Tick(18),
                },
            );
        }
        assert_eq!(
            session.advance(Tick(17), true),
            AdvanceResult::Started(Tick(18))
        );
        assert_eq!(
            session.state(),
            P2PSessionState::Started {
                start_tick: Tick(18)
            }
        );
    }

    #[test]
    fn a_different_roster_fails_the_barrier() {
        let local = peer(0);
        let remote_peers = SmallVec::from_slice(&[peer(1)]);
        let expected_roster_hash = roster_hash(local, &remote_peers);
        let mut session = P2PSession::default();
        session.begin(remote_peers, expected_roster_hash, Duration::ZERO);

        session.receive(
            peer(1),
            P2PSessionMessage::Ready {
                generation: session.generation,
                roster_hash: roster_hash(local, &[peer(1), peer(2)]),
                earliest_start_tick: Tick(10),
            },
        );

        assert_eq!(session.advance(Tick(0), true), AdvanceResult::Failed);
    }

    #[test]
    fn roster_hash_is_independent_of_link_order() {
        assert_eq!(
            roster_hash(peer(0), &[peer(1), peer(2), peer(3)]),
            roster_hash(peer(2), &[peer(3), peer(0), peer(1)])
        );
    }

    #[test]
    fn a_start_attempt_times_out() {
        let remote_peers = SmallVec::from_slice(&[peer(1)]);
        assert_eq!(P2PSession::default().start_timeout, Duration::from_secs(5));
        let mut session = P2PSession::default().with_start_timeout(Duration::from_secs(2));
        session.begin(
            remote_peers,
            roster_hash(peer(0), &[peer(1)]),
            Duration::from_secs(5),
        );

        assert!(!session.timed_out_at(Duration::from_secs(6)));
        assert!(session.timed_out_at(Duration::from_secs(7)));
    }

    #[derive(Resource, Default)]
    struct WasStopped(bool);

    fn record_stopped(_trigger: On<P2PStopped>, mut stopped: ResMut<WasStopped>) {
        stopped.0 = true;
    }

    #[test]
    fn stop_keeps_links_and_emits_stopped() {
        let mut app = App::new();
        app.add_plugins(lightyear_link::LinkPlugin);
        let link = app.world_mut().spawn(P2P::Joined).id();
        let mut session = P2PSession::default();
        session.begin(
            SmallVec::from_slice(&[peer(1)]),
            roster_hash(peer(0), &[peer(1)]),
            Duration::ZERO,
        );
        app.insert_resource(session);
        app.init_resource::<WasStopped>();
        app.add_observer(stop_session);
        app.add_observer(record_stopped);

        app.world_mut().trigger(P2PStop { unlink: false });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<P2PSession>().state(),
            P2PSessionState::Stopped
        );
        assert!(app.world().entity(link).contains::<P2P>());
        assert_eq!(app.world().entity(link).get::<P2P>(), Some(&P2P::Inactive));
        assert!(
            !app.world()
                .entity(link)
                .contains::<lightyear_link::prelude::Unlinked>()
        );
        assert!(app.world().resource::<WasStopped>().0);
    }

    #[test]
    fn stop_can_unlink_every_declared_p2p_link() {
        let mut app = App::new();
        app.add_plugins(lightyear_link::LinkPlugin);
        let cohort_link = app.world_mut().spawn(P2P::Joined).id();
        let late_link = app.world_mut().spawn(P2P::Inactive).id();
        let mut session = P2PSession::default();
        session.begin(
            SmallVec::from_slice(&[peer(1)]),
            roster_hash(peer(0), &[peer(1)]),
            Duration::ZERO,
        );
        app.insert_resource(session);
        app.add_observer(stop_session);

        app.world_mut().trigger(P2PStop { unlink: true });
        app.world_mut().flush();

        assert!(
            app.world()
                .entity(cohort_link)
                .contains::<lightyear_link::prelude::Unlinked>()
        );
        assert!(
            app.world()
                .entity(late_link)
                .contains::<lightyear_link::prelude::Unlinked>()
        );
    }
}
