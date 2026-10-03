use alloc::vec::Vec;
use bevy_app::{App, FixedFirst, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use core::hash::Hasher;
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_connection::network_topology::{
    NetworkTopology, NetworkTopologySystems, NetworkingMetadata,
};
use lightyear_connection::p2p::P2P;
use lightyear_connection::p2p::{P2PRoster, P2PSessionPhase};

use crate::join::{JoinDriveParams, P2PJoinState, PendingActivation};
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_core::prelude::{LocalTimeline, Tick, TimelineSystems};
use lightyear_link::prelude::{Unlink, UnlinkReason};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageReceiver, MessageSender};
use lightyear_serde::ToBytes;
use lightyear_sync::plugin::SyncSystems;
use lightyear_sync::prelude::SyncedLocalTimeline;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

const DEFAULT_START_DELAY_TICKS: u16 = 120;
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MIN_PLAYERS: u8 = 2;

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
/// [`P2PSessionPlugin`] initializes this resource, so an application that wants non-default policy
/// inserts a configured value *before* adding the plugin:
///
/// ```ignore
/// app.insert_resource(
///     P2PSession::default()
///         .with_min_players(1)
///         .with_start_delay_ticks(60),
/// )
/// .add_plugins(P2PSessionPlugin);
/// ```
///
/// Applications declare [`P2P`] Links and trigger [`P2PStart`] with the desired cohort.
/// The session freezes the selected peer identities for the barrier; it does not retain the
/// event's selection policy.
#[derive(Resource, Debug, Clone)]
pub struct P2PSession {
    /// Smallest number of players, including the local peer, that may start a session.
    min_players: u8,
    /// Lead added to the local tick when this peer proposes a shared future tick.
    start_delay_ticks: u16,
    /// Maximum wall-clock time allowed for one start attempt.
    pub(crate) start_timeout: Duration,
    /// Wall-clock timestamp at which the current start attempt began.
    start_started_at: Option<Duration>,
    /// Current deterministic-session lifecycle.
    pub(crate) state: P2PSessionState,
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
    /// Admission, catch-up negotiation, and scheduled membership activation.
    pub(crate) join: P2PJoinState,
    /// This application's own peer id, learned from a declared Link.
    pub(crate) local_peer_id: Option<PeerId>,
}

impl Default for P2PSession {
    fn default() -> Self {
        Self {
            min_players: DEFAULT_MIN_PLAYERS,
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
            join: P2PJoinState::default(),
            local_peer_id: None,
        }
    }
}

impl P2PSession {
    /// Set the smallest number of players, including the local peer, that may start a session.
    ///
    /// The default of `2` refuses a start with no remote candidate, which is a mistake for an
    /// application that declared P2P Links but has not connected them: it would replace a
    /// conventional topology with a solo P2P one. A value of `1` opts into starting alone.
    pub fn with_min_players(mut self, min_players: u8) -> Self {
        self.min_players = min_players.max(1);
        self
    }

    /// The smallest roster, including the local peer, that may start a session.
    ///
    /// An application that waits for more peers than this is choosing to; the session itself only
    /// requires this many, because a peer that is not here yet can join a running session.
    pub fn min_players(&self) -> u8 {
        self.min_players
    }

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

    /// Membership generation of this session.
    ///
    /// It starts at zero for the initial cohort and increments on every committed membership
    /// change, so it identifies *which* session membership a control message belongs to.
    pub(crate) fn epoch(&self) -> u32 {
        self.join.epoch
    }

    /// The peers playing in this session, excluding the local peer.
    ///
    /// The committed roster: peers that have crossed the barrier and are not in the middle of being
    /// admitted.
    pub fn started_peers(&self) -> SmallVec<[PeerId; 4]> {
        if let Some(attempt) = &self.join.attempt {
            return attempt.hosts.clone();
        }
        self.remote_peers.iter().map(|peer| peer.peer_id).collect()
    }

    /// This application's own peer id, once a declared Link has revealed it.
    pub(crate) fn local_peer_id(&self) -> Option<PeerId> {
        self.local_peer_id
    }

    /// Whether a join is in flight on this application.
    pub(crate) fn is_joining(&self) -> bool {
        self.join.attempt.is_some()
    }

    /// Lead added to the local tick when this peer proposes a shared future tick.
    pub(crate) fn start_delay_ticks(&self) -> u16 {
        self.start_delay_ticks
    }

    /// Wall-clock budget for one negotiation attempt.
    pub(crate) fn start_timeout(&self) -> Duration {
        self.start_timeout
    }

    /// Record a peer id revealed by a declared Link.
    pub(crate) fn observe_local_id(&mut self, local_id: PeerId) {
        self.local_peer_id.get_or_insert(local_id);
    }

    /// Apply the agreed membership and gameplay change at one activation boundary.
    pub(crate) fn apply_activation(&mut self, activation: PendingActivation) {
        self.join.epoch = activation.epoch.wrapping_add(1);
        if activation.local_is_joiner {
            let attempt = self
                .join
                .attempt
                .take()
                .expect("joining peer has an attempt");
            self.remote_peers = attempt
                .hosts
                .into_iter()
                .map(RemotePeerStart::new)
                .collect();
            self.state = P2PSessionState::Started {
                start_tick: attempt
                    .session_start_tick
                    .expect("admitted session has a start tick"),
            };
        } else if !self
            .remote_peers
            .iter()
            .any(|peer| peer.peer_id == activation.peer_id)
        {
            self.remote_peers
                .push(RemotePeerStart::new(activation.peer_id));
        }
        self.join.admission = None;
        self.join.activation = None;
    }

    /// Reset negotiation state and freeze the supplied remote identities into a new start attempt.
    pub(crate) fn begin(
        &mut self,
        remote_peer_ids: SmallVec<[PeerId; 4]>,
        roster_hash: Option<u64>,
        started_at: Duration,
    ) {
        self.generation = self.generation.wrapping_add(1);
        self.start_started_at = Some(started_at);
        self.local_ready_tick = None;
        self.local_ready_sent = false;
        self.local_acknowledgement_sent = false;
        self.roster_hash = roster_hash;
        self.remote_peers = remote_peer_ids
            .into_iter()
            .map(RemotePeerStart::new)
            .collect();
        self.roster_mismatch = false;
        self.state = P2PSessionState::Starting { start_tick: None };
    }

    /// Clear the active cohort and return to the lobby/stopped state.
    pub(crate) fn stop(&mut self) {
        self.state = P2PSessionState::Stopped;
        self.clear_barrier_progress();
        self.remote_peers.clear();
        self.join.clear();
    }

    /// Drop transient barrier bookkeeping while preserving the public Started state.
    ///
    /// The committed roster is kept: it is the session's membership, and a later join reads it to
    /// tell a newcomer which peers are playing.
    pub(crate) fn clear_barrier_progress(&mut self) {
        self.start_started_at = None;
        self.local_ready_tick = None;
        self.local_ready_sent = false;
        self.local_acknowledgement_sent = false;
        self.roster_hash = None;
        self.roster_mismatch = false;
    }

    fn contains_remote(&self, peer_id: PeerId) -> bool {
        self.remote_peers.iter().any(|peer| peer.peer_id == peer_id)
    }

    fn timed_out_at(&self, now: Duration, start_timeout: Duration) -> bool {
        self.start_started_at
            .is_some_and(|started_at| now.saturating_sub(started_at) >= start_timeout)
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

    /// Advance the local start negotiation through agreement on a future start tick.
    ///
    /// Each peer proposes `current tick + start delay`. Once all proposals exist, their maximum
    /// is the common start tick. Every peer must then acknowledge that same value. The transition
    /// to Started happens separately in `FixedFirst`, immediately before the [`LocalTimeline`]
    /// advances to the agreed tick.
    fn advance(
        &mut self,
        tick: Tick,
        timeline_synced: bool,
        start_delay_ticks: u16,
    ) -> AdvanceResult {
        if !matches!(self.state, P2PSessionState::Starting { .. }) {
            return AdvanceResult::Waiting;
        }
        if self.roster_mismatch {
            return AdvanceResult::Failed;
        }

        // Do not advertise readiness until the shared input timeline is usable.
        if timeline_synced && self.local_ready_tick.is_none() {
            self.local_ready_tick = Some(tick + i32::from(start_delay_ticks));
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

        // PreUpdate observes the tick most recently simulated by FixedMain. Consensus reached at
        // or after the target tick is too late to create the shared world before that tick.
        if tick >= start_tick {
            tracing::warn!(
                ?tick,
                ?start_tick,
                "P2P start acknowledgement arrived after the agreed tick"
            );
            return AdvanceResult::Failed;
        }
        AdvanceResult::Agreed(start_tick)
    }

    /// Return the start tick once every remote peer has acknowledged it.
    fn agreed_start_tick(&self) -> Option<Tick> {
        let P2PSessionState::Starting {
            start_tick: Some(start_tick),
        } = self.state
        else {
            return None;
        };
        if self
            .remote_peers
            .iter()
            .any(|peer| peer.acknowledged_tick != Some(start_tick))
        {
            return None;
        }
        Some(start_tick)
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

/// Start a deterministic session with the selected declared P2P Links.
///
/// The Links need not be connected yet. Every peer must select the same cohort; Lightyear freezes
/// its identities, waits for the Links and synchronized input timeline, then agrees a future tick.
/// Selection is explicit rather than inferred from connectivity, which can differ between peers.
/// The default selects all declared inactive Links. Excluded Links remain available for later joins.
#[derive(Event, Debug, Clone, PartialEq)]
pub struct P2PStart {
    /// Remote peers to include in this start attempt.
    pub cohort: NetworkTarget,
}

impl Default for P2PStart {
    fn default() -> Self {
        Self {
            cohort: NetworkTarget::All,
        }
    }
}

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

/// Reliable control channel shared by P2P startup, admission, and activation.
pub struct P2PChannel;

/// Registers the private P2P control protocol.
///
/// This plugin is installed by Lightyear's shared plugin setup so P2P-enabled conventional
/// clients and servers reserve identical channel and message IDs. [`P2PSessionPlugin`] installs
/// it as a fallback for applications that use this crate directly.
#[doc(hidden)]
pub struct P2PProtocolPlugin;

impl Plugin for P2PProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<P2PChannel>(ChannelSettings {
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
        if !app.is_plugin_added::<crate::P2PJoinPlugin>() {
            app.add_plugins(crate::P2PJoinPlugin);
        }
        // `init_resource` keeps an application-provided session, so policy can be set through
        // `P2PSession`'s builders before this plugin is added. `P2PSessionPhase` belongs to
        // `ConnectionPlugin`; reach for it directly in case this plugin is used standalone.
        app.init_resource::<P2PSessionPhase>();
        app.init_resource::<P2PSession>();

        app.add_observer(start_session);
        app.add_observer(stop_session);
        app.add_systems(PreUpdate, drive_session.after(MessageSystems::Receive));
        // PreUpdate runs only once per rendered frame, which may contain several catch-up fixed
        // ticks. FixedFirst observes every boundary, so it cannot skip the transition to the
        // agreed tick. P2PStarted must run before IncrementLocal: a late input for the first
        // gameplay tick T rolls back to the shared snapshot at T - 1.
        app.add_systems(
            FixedFirst,
            start_session_before_agreed_tick.before(TimelineSystems::IncrementLocal),
        );
        // Publish the session lifecycle to the layers below this crate. PostUpdate runs after both
        // PreUpdate and the fixed schedules, so every transition this frame is visible before the
        // topology projection, timeline synchronization, and input routing read it.
        app.add_systems(
            PostUpdate,
            sync_session_phase
                .before(NetworkTopologySystems::Update)
                .before(SyncSystems::Sync),
        );
    }
}

/// Publish the session lifecycle where the layers below this crate can read it.
///
/// [`NetworkTopology`], timeline synchronization, and input routing cannot read [`P2PSession`],
/// which lives above them, so they branch on [`P2PSessionPhase`] instead.
fn sync_session_phase(session: Res<P2PSession>, mut phase: ResMut<P2PSessionPhase>) {
    // An attempt to join a session that is already running is its own lifecycle: the application has
    // no session of its own yet, but it must follow the cohort's clock and receive its inputs.
    let next = if session.is_joining() {
        P2PSessionPhase::Joining
    } else {
        match session.state() {
            P2PSessionState::Stopped => P2PSessionPhase::Stopped,
            P2PSessionState::Starting { .. } => P2PSessionPhase::Starting,
            P2PSessionState::Started { .. } => P2PSessionPhase::Active,
        }
    };
    // Only write on a real transition: a `DerefMut` would mark the resource as changed and make
    // the topology projection re-run on every frame.
    if *phase != next {
        *phase = next;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvanceResult {
    Waiting,
    Agreed(Tick),
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

/// Freeze the selected remote identities and begin their start barrier.
fn start_session(
    trigger: On<P2PStart>,
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

    // The cohort is the Links the application selected, not every Link it declared.
    //
    // A Link left out keeps `P2P::Inactive`, which is the state the join protocol admits from, so a
    // peer the application expects later neither holds the barrier up nor loses the Link it will
    // need.
    let cohort = &trigger.cohort;
    let candidates: SmallVec<[(Entity, Option<PeerId>, Option<PeerId>, bool); 4]> = links
        .iter()
        .filter_map(|(entity, state, local_id, remote_id, has_receiver)| {
            let remote = remote_id.map(|id| id.0)?;
            (*state == P2P::Inactive && cohort.matches(&remote)).then_some((
                entity,
                local_id.map(|id| id.0),
                Some(remote),
                has_receiver,
            ))
        })
        .collect();
    let peer_count = candidates.len().saturating_add(1);
    if candidates.is_empty() && session.min_players > 1 {
        tracing::warn!(
            min_players = session.min_players,
            "P2PStart requires at least one declared remote P2P Link in the start cohort"
        );
        return;
    }

    // A solo session has no Link to read the local identity from, and no roster to agree on: the
    // roster hash stays unset and no session message is ever queued, because `collect_outbound`
    // requires a hash. `min_players == 1` is what makes an empty cohort legal.
    let local_id = candidates.first().and_then(|(_, local_id, _, _)| *local_id);
    let mut remote_ids = SmallVec::<[PeerId; 4]>::new();
    if !candidates.is_empty() {
        let Some(local_id) = local_id else {
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
    }
    let roster_hash = local_id.map(|local_id| roster_hash(local_id, &remote_ids));

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
    let was_running = !matches!(session.state, P2PSessionState::Stopped)
        || session.is_joining()
        || session.join.activation.is_some();
    session.stop();

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

/// Complete an acknowledged barrier immediately before its first gameplay tick.
fn start_session_before_agreed_tick(
    mut commands: Commands,
    mut session: ResMut<P2PSession>,
    timeline: Res<LocalTimeline>,
    mut metadata: ResMut<NetworkingMetadata>,
    mut links: P2PLinkQuery,
    mut phase: ResMut<P2PSessionPhase>,
) {
    // A scheduled membership change applies at its own tick, independent of the start barrier.
    crate::join::apply_join_activation_inner(
        &mut commands,
        &mut session,
        &timeline,
        &metadata,
        &mut phase,
    );
    let Some(start_tick) = session.agreed_start_tick() else {
        return;
    };
    if timeline.tick() + 1 != start_tick {
        return;
    }

    let mut candidate_count = 0;
    let mut joined = SmallVec::<[Entity; 4]>::new();
    for (entity, state, _, connected, _, _) in &mut links {
        if *state == P2P::Candidate {
            candidate_count += 1;
            if !connected {
                return;
            }
            joined.push(entity);
        }
    }
    if candidate_count != session.remote_peers.len() {
        return;
    }

    joined.sort_unstable_by_key(|entity| entity.index_u32());
    session.state = P2PSessionState::Started { start_tick };
    transition_candidates(&mut commands, &mut links, P2P::Joined);
    // Publish the started session to this frame's fixed ticks without waiting for the PostUpdate
    // projection. Every link the barrier admitted is now a started peer.
    metadata.mode = NetworkTopology::P2P(P2PRoster {
        phase: P2PSessionPhase::Active,
        started: joined,
        ..Default::default()
    });
    tracing::info!(?start_tick, "P2P session started");
    session.clear_barrier_progress();
    commands.trigger(P2PStarted { start_tick });
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
    join: JoinDriveParams,
) {
    // Admission runs whether or not a start barrier is in flight.
    crate::join::drive_join_inner(
        &mut commands,
        &mut session,
        &real_time,
        &timeline,
        synced_timeline.is_some(),
        join,
    );
    if !matches!(session.state, P2PSessionState::Starting { .. }) {
        return;
    }

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
    if !all_connected {
        if session.timed_out_at(real_time.elapsed(), session.start_timeout) {
            let timeout = session.start_timeout;
            session.stop();
            transition_candidates(&mut commands, &mut links, P2P::Inactive);
            tracing::warn!(?timeout, "P2P session start negotiation timed out");
        }
        return;
    }
    let start_delay_ticks = session.start_delay_ticks;
    match session.advance(
        timeline.tick(),
        synced_timeline.is_some(),
        start_delay_ticks,
    ) {
        AdvanceResult::Failed => {
            tracing::warn!("P2P session start negotiation failed");
            session.stop();
            transition_candidates(&mut commands, &mut links, P2P::Inactive);
            return;
        }
        AdvanceResult::Agreed(start_tick) => {
            tracing::trace!(?start_tick, "P2P session start tick agreed");
        }
        AdvanceResult::Waiting => {}
    }

    // Once every remote has acknowledged the common tick, the negotiation is complete. Do not
    // let its wall-clock timeout expire while FixedFirst waits for that future tick boundary.
    if session.agreed_start_tick().is_none()
        && session.timed_out_at(real_time.elapsed(), session.start_timeout)
    {
        let timeout = session.start_timeout;
        session.stop();
        transition_candidates(&mut commands, &mut links, P2P::Inactive);
        tracing::warn!(?timeout, "P2P session start negotiation timed out");
        return;
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
            sender.send::<P2PChannel>(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_connection::network_target::TargetList;

    fn peer(index: u64) -> PeerId {
        PeerId::Entity(index)
    }

    #[test]
    fn start_freezes_the_cohort_and_ignores_links_declared_later() {
        let mut app = App::new();
        app.init_resource::<P2PSession>();
        app.init_resource::<Time<Real>>();
        app.add_observer(start_session);
        let local_id = peer(0);
        let second = app
            .world_mut()
            .spawn((
                P2P::Inactive,
                LocalId(local_id),
                RemoteId(peer(2)),
                Connected,
            ))
            .id();
        let first = app
            .world_mut()
            .spawn((
                P2P::Inactive,
                LocalId(local_id),
                RemoteId(peer(1)),
                Connected,
            ))
            .id();

        app.world_mut().trigger(P2PStart::default());
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

        app.world_mut().trigger(P2PStart::default());
        app.world_mut().flush();
        assert_eq!(
            app.world().resource::<P2PSession>().generation,
            generation,
            "a duplicate start must not restart or expand the active barrier"
        );

        let late = app
            .world_mut()
            .spawn((
                P2P::Inactive,
                LocalId(local_id),
                RemoteId(peer(3)),
                Connected,
            ))
            .id();
        assert_eq!(app.world().entity(late).get::<P2P>(), Some(&P2P::Inactive));
        assert!(
            !app.world()
                .resource::<P2PSession>()
                .contains_remote(peer(3))
        );
    }

    /// A Link outside the start cohort is left alone, and stays available for a join.
    ///
    /// The cohort has to be declared rather than inferred: the barrier agrees by comparing a roster
    /// hash, so two peers that judged a peer's presence at different moments would derive different
    /// cohorts and fail it. For plain UDP there is nothing to infer from anyway — a socket reports
    /// itself connected the moment it binds, whether or not anything is at the other end.
    #[test]
    fn a_link_outside_the_start_cohort_is_left_for_the_join_protocol() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>();
        app.add_observer(start_session);
        let local_id = peer(0);
        // Only peer 1 is expected to start; peer 2 is expected to arrive later and join.
        app.init_resource::<P2PSession>();
        let starting = app
            .world_mut()
            .spawn((P2P::Inactive, LocalId(local_id), RemoteId(peer(1))))
            .id();
        let later = app
            .world_mut()
            .spawn((P2P::Inactive, LocalId(local_id), RemoteId(peer(2))))
            .id();

        app.world_mut().trigger(P2PStart {
            cohort: NetworkTarget::Only(TargetList::from_slice(&[peer(1)])),
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().entity(starting).get::<P2P>(),
            Some(&P2P::Candidate)
        );
        assert_eq!(
            app.world().entity(later).get::<P2P>(),
            Some(&P2P::Inactive),
            "a peer outside the cohort must be left for the join protocol, not counted in it"
        );
        let session = app.world().resource::<P2PSession>();
        assert_eq!(session.remote_peers.len(), 1);
        assert!(session.contains_remote(peer(1)));
        assert!(!session.contains_remote(peer(2)));
    }

    #[test]
    fn the_session_phase_tracks_the_lifecycle() {
        #[derive(Resource, Default)]
        struct Phases(Vec<P2PSessionPhase>);

        let mut app = App::new();
        app.init_resource::<P2PSession>();
        app.init_resource::<P2PSessionPhase>();
        app.init_resource::<Phases>();
        app.add_systems(PostUpdate, sync_session_phase);
        app.add_systems(
            PostUpdate,
            (|phase: Res<P2PSessionPhase>, mut phases: ResMut<Phases>| {
                phases.0.push(*phase);
            })
            .after(sync_session_phase),
        );

        app.update();
        assert_eq!(
            app.world().resource::<Phases>().0.as_slice(),
            &[P2PSessionPhase::Stopped]
        );

        // A forming cohort is a start candidate, not a started peer.
        app.world_mut().resource_mut::<P2PSession>().state =
            P2PSessionState::Starting { start_tick: None };
        app.update();
        let phase = *app.world().resource::<Phases>().0.last().unwrap();
        assert_eq!(phase, P2PSessionPhase::Starting);

        // Crossing the barrier makes it a started peer.
        app.world_mut().resource_mut::<P2PSession>().state = P2PSessionState::Started {
            start_tick: Tick(10),
        };
        app.update();
        let phase = *app.world().resource::<Phases>().0.last().unwrap();
        assert_eq!(phase, P2PSessionPhase::Active);

        // Stopping clears it.
        app.world_mut().resource_mut::<P2PSession>().state = P2PSessionState::Stopped;
        app.update();
        assert_eq!(
            *app.world().resource::<Phases>().0.last().unwrap(),
            P2PSessionPhase::Stopped
        );
    }

    #[test]
    fn ready_peers_agree_on_the_latest_tick() {
        let mut session = P2PSession::default();
        let local = peer(0);
        let first = peer(1);
        let second = peer(2);
        let remote_peers = SmallVec::from_slice(&[first, second]);
        let roster_hash = roster_hash(local, &remote_peers);
        session.begin(remote_peers, Some(roster_hash), Duration::ZERO);

        assert_eq!(session.advance(Tick(5), false, 10), AdvanceResult::Waiting);
        assert_eq!(session.advance(Tick(6), true, 10), AdvanceResult::Waiting);
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
        assert_eq!(session.advance(Tick(7), true, 10), AdvanceResult::Waiting);
        assert_eq!(session.start_tick(), Some(Tick(18)));
        let mut outbound = SmallVec::new();
        session.collect_outbound(&mut outbound);

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
            session.advance(Tick(17), true, 10),
            AdvanceResult::Agreed(Tick(18))
        );
        assert_eq!(
            session.state(),
            P2PSessionState::Starting {
                start_tick: Some(Tick(18))
            }
        );
        assert_eq!(session.agreed_start_tick(), Some(Tick(18)));
    }

    #[test]
    fn a_different_roster_fails_the_barrier() {
        let local = peer(0);
        let remote_peers = SmallVec::from_slice(&[peer(1)]);
        let expected_roster_hash = roster_hash(local, &remote_peers);
        let mut session = P2PSession::default();
        session.begin(remote_peers, Some(expected_roster_hash), Duration::ZERO);

        session.receive(
            peer(1),
            P2PSessionMessage::Ready {
                generation: session.generation,
                roster_hash: roster_hash(local, &[peer(1), peer(2)]),
                earliest_start_tick: Tick(10),
            },
        );

        assert_eq!(session.advance(Tick(0), true, 0), AdvanceResult::Failed);
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
        let timeout = Duration::from_secs(2);
        assert_eq!(P2PSession::default().start_timeout, Duration::from_secs(5));
        let mut session = P2PSession::default();
        session.begin(
            remote_peers,
            Some(roster_hash(peer(0), &[peer(1)])),
            Duration::from_secs(5),
        );

        assert!(!session.timed_out_at(Duration::from_secs(6), timeout));
        assert!(session.timed_out_at(Duration::from_secs(7), timeout));
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
            Some(roster_hash(peer(0), &[peer(1)])),
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
            Some(roster_hash(peer(0), &[peer(1)])),
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
