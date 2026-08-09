use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_topology::NetworkTopologySystems;
use lightyear_connection::p2p::P2P;
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_link::prelude::{Unlink, UnlinkReason};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageReceiver, MessageSender};
use lightyear_sync::prelude::SyncedLocalTimeline;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

const DEFAULT_START_DELAY_TICKS: u16 = 120;

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

/// Start-negotiation progress for one remote peer in the frozen cohort.
#[derive(Debug, Clone, Copy)]
struct RemotePeerStart {
    /// Local entity of the direct Link to this remote peer.
    link: Entity,
    /// Earliest start tick advertised by this remote peer.
    ready_tick: Option<Tick>,
    /// Start tick this remote peer acknowledged after seeing every Ready message.
    acknowledged_tick: Option<Tick>,
}

impl RemotePeerStart {
    fn new(link: Entity) -> Self {
        Self {
            link,
            ready_tick: None,
            acknowledged_tick: None,
        }
    }
}

/// Application-global configuration and state for one deterministic P2P session.
///
/// Lightyear does not discover or connect peers through this resource. Applications declare
/// [`P2P`] Links normally, then trigger [`P2PStart`] when the desired initial cohort has been
/// declared. The Links don't have to be connected yet when [`P2PStart`] is called.
#[derive(Resource, Debug)]
pub struct P2PSession {
    /// Maximum cohort size, including the local peer.
    max_peers: u8,
    /// Application-facing policy indicating whether new Links may be established after start.
    allow_late_join: bool,
    /// Lead added to the local tick when this peer becomes ready.
    start_delay_ticks: u16,
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
    /// Remote peers frozen by [`P2PStart`], one per direct P2P Link.
    ///
    /// The local peer is not included; its progress is stored in the `local_*` fields above.
    remote_peers: SmallVec<[RemotePeerStart; 4]>,
}

impl P2PSession {
    /// Create a stopped session with capacity for `max_peers`, including the local peer.
    ///
    /// A P2P session needs at least two peers. The count is a capacity, not a required roster
    /// size: the application decides when enough Links exist and triggers [`P2PStart`].
    pub fn new(max_peers: u8) -> Self {
        assert!(max_peers >= 2, "a P2P session needs at least two peers");
        Self {
            max_peers,
            allow_late_join: false,
            start_delay_ticks: DEFAULT_START_DELAY_TICKS,
            state: P2PSessionState::Stopped,
            generation: 0,
            local_ready_tick: None,
            local_ready_sent: false,
            local_acknowledgement_sent: false,
            remote_peers: SmallVec::new(),
        }
    }

    /// Allow the application to establish additional P2P Links after the session starts.
    ///
    /// This is an application-facing admission policy; the session does not create or reject
    /// Links. A late Link is not added to the running deterministic cohort automatically. It can
    /// receive traffic and perform application-owned catch-up, then participate in a later start.
    pub fn with_late_join(mut self, allow: bool) -> Self {
        self.allow_late_join = allow;
        self
    }

    /// Set the lead time used when proposing a common future start tick.
    pub fn with_start_delay_ticks(mut self, ticks: u16) -> Self {
        self.start_delay_ticks = ticks;
        self
    }

    /// Maximum peer count, including the local peer.
    pub fn max_peers(&self) -> u8 {
        self.max_peers
    }

    /// Whether the application may establish Links after this session starts.
    pub fn allows_late_join(&self) -> bool {
        self.allow_late_join
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

    /// Links frozen into the current starting or started cohort.
    pub fn links(&self) -> impl Iterator<Item = Entity> + '_ {
        self.remote_peers.iter().map(|peer| peer.link)
    }

    /// Reset negotiation state and freeze the supplied remote Links into a new start attempt.
    fn begin(&mut self, mut links: SmallVec<[Entity; 4]>) {
        links.sort_unstable();
        self.generation = self.generation.wrapping_add(1);
        self.local_ready_tick = None;
        self.local_ready_sent = false;
        self.local_acknowledgement_sent = false;
        self.remote_peers = links.into_iter().map(RemotePeerStart::new).collect();
        self.state = P2PSessionState::Starting { start_tick: None };
    }

    /// Clear the active cohort and return to the lobby/stopped state.
    fn stop(&mut self) {
        self.state = P2PSessionState::Stopped;
        self.local_ready_tick = None;
        self.local_ready_sent = false;
        self.local_acknowledgement_sent = false;
        self.remote_peers.clear();
    }

    /// Record one message received from a remote Link in the current start attempt.
    ///
    /// Delayed messages from older generations and messages from Links outside the frozen cohort
    /// are ignored. Repeated identical messages are harmless; conflicting repeats are logged and
    /// the first value remains authoritative.
    fn receive(&mut self, link: Entity, message: P2PSessionMessage) {
        // Reliable packets from a previous start can arrive after a stop/restart cycle.
        let generation = match message {
            P2PSessionMessage::Ready { generation, .. }
            | P2PSessionMessage::StartAcknowledgement { generation, .. } => generation,
        };
        if generation != self.generation {
            return;
        }
        let Some(peer) = self.remote_peers.iter_mut().find(|peer| peer.link == link) else {
            return;
        };
        match message {
            P2PSessionMessage::Ready {
                earliest_start_tick,
                ..
            } => match peer.ready_tick {
                None => peer.ready_tick = Some(earliest_start_tick),
                Some(previous) if previous != earliest_start_tick => tracing::warn!(
                    ?link,
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
                        ?link,
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
        if let Some((link, acknowledged)) = self.remote_peers.iter().find_map(|peer| {
            peer.acknowledged_tick
                .filter(|acknowledged| *acknowledged != start_tick)
                .map(|acknowledged| (peer.link, acknowledged))
        }) {
            tracing::warn!(
                ?link,
                ?start_tick,
                ?acknowledged,
                "peer acknowledged a different P2P start tick"
            );
            self.stop();
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
            self.stop();
            return AdvanceResult::Failed;
        }
        if tick + 1 < start_tick {
            return AdvanceResult::Waiting;
        }
        self.state = P2PSessionState::Started { start_tick };
        AdvanceResult::Started(start_tick)
    }

    /// Queue each locally available Ready or acknowledgement once for every remote Link.
    ///
    /// The transport channel provides retransmission; these flags only prevent this system from
    /// enqueuing an identical message every frame.
    fn collect_outbound(&mut self, outbound: &mut SmallVec<[(Entity, P2PSessionMessage); 8]>) {
        if let Some(earliest_start_tick) = self.local_ready_tick
            && !self.local_ready_sent
        {
            self.local_ready_sent = true;
            for peer in &self.remote_peers {
                outbound.push((
                    peer.link,
                    P2PSessionMessage::Ready {
                        generation: self.generation,
                        earliest_start_tick,
                    },
                ));
            }
        }
        if let Some(start_tick) = self.start_tick()
            && !self.local_acknowledgement_sent
        {
            self.local_acknowledgement_sent = true;
            for peer in &self.remote_peers {
                outbound.push((
                    peer.link,
                    P2PSessionMessage::StartAcknowledgement {
                        generation: self.generation,
                        start_tick,
                    },
                ));
            }
        }
    }
}

/// Trigger this locally on every peer after declaring the P2P Links that should form the initial
/// session cohort.
///
/// The Links don't have to be connected yet when `P2PStart` is called. Lightyear waits for every
/// captured Link and the synchronized input timeline, negotiates a common future tick, then
/// triggers [`P2PStarted`].
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

/// Triggered at the agreed tick, immediately before the fixed simulation schedule.
///
/// Applications can observe this to create the shared deterministic world in the same order on
/// every peer.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PStarted {
    /// Common tick at which deterministic play begins.
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
        /// Sender's current tick plus its configured start delay.
        earliest_start_tick: Tick,
    },
    /// Confirm the maximum Ready tick selected as the common start tick.
    StartAcknowledgement {
        /// Start-attempt number used to discard delayed messages from earlier attempts.
        generation: u32,
        /// Common start tick calculated by the sender.
        start_tick: Tick,
    },
}

/// Private reliable delivery queue for P2P lifecycle control messages.
struct P2PSessionChannel;

/// Installs the P2P session lifecycle and start-tick negotiation.
pub struct P2PSessionPlugin;

impl Plugin for P2PSessionPlugin {
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

        app.add_observer(start_session);
        app.add_observer(stop_session);
        app.add_systems(
            PreUpdate,
            drive_session
                .after(MessageSystems::Receive)
                .after(NetworkTopologySystems::Update),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvanceResult {
    Waiting,
    Started(Tick),
    Failed,
}

/// Freeze every currently declared P2P Link as the remote cohort for a new start attempt.
///
/// This validates the configured capacity and installs the private typed receiver, but does not
/// connect the Links. [`drive_session`] waits for connection and timeline readiness.
fn start_session(
    _trigger: On<P2PStart>,
    mut commands: Commands,
    mut session: Option<ResMut<P2PSession>>,
    links: Query<(Entity, Has<MessageReceiver<P2PSessionMessage>>), With<P2P>>,
) {
    let Some(session) = session.as_deref_mut() else {
        tracing::warn!("P2PStart requires a P2PSession resource");
        return;
    };
    if !matches!(session.state, P2PSessionState::Stopped) {
        tracing::warn!("ignoring P2PStart because the session is not stopped");
        return;
    }

    let declared_links: SmallVec<[(Entity, bool); 4]> = links.iter().collect();
    let peer_count = declared_links.len().saturating_add(1);
    if declared_links.is_empty() {
        tracing::warn!("P2PStart requires at least one declared remote P2P Link");
        return;
    }
    if peer_count > usize::from(session.max_peers) {
        tracing::warn!(
            peer_count,
            max_peers = session.max_peers,
            "P2PStart exceeds the configured peer capacity"
        );
        return;
    }
    for (entity, has_receiver) in &declared_links {
        if !has_receiver {
            commands
                .entity(*entity)
                .insert(MessageReceiver::<P2PSessionMessage>::default());
        }
    }
    let links = declared_links
        .into_iter()
        .map(|(entity, _)| entity)
        .collect();

    tracing::info!(peer_count, "starting P2P session negotiation");
    session.begin(links);
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
    mut session: Option<ResMut<P2PSession>>,
    links: Query<Entity, With<P2P>>,
) {
    let Some(session) = session.as_deref_mut() else {
        tracing::warn!("P2PStop requires a P2PSession resource");
        return;
    };
    let was_running = !matches!(session.state, P2PSessionState::Stopped);
    if was_running {
        session.stop();
    }

    if trigger.unlink {
        for entity in &links {
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
        Has<Connected>,
        Option<&'static mut MessageSender<P2PSessionMessage>>,
        Option<&'static mut MessageReceiver<P2PSessionMessage>>,
    ),
    With<P2P>,
>;

/// Drive the current start attempt once per frame before fixed simulation.
///
/// Incoming messages update per-remote-peer progress. Once every frozen Link is connected and the
/// input timeline is synchronized, [`P2PSession::advance`] chooses and acknowledges the start
/// tick. Newly available local messages are then queued once on every remote Link.
fn drive_session(
    mut commands: Commands,
    mut session: Option<ResMut<P2PSession>>,
    timeline: Res<LocalTimeline>,
    synced_timeline: Option<SyncedLocalTimeline>,
    mut links: P2PLinkQuery,
) {
    let Some(session) = session.as_deref_mut() else {
        return;
    };
    if !matches!(session.state, P2PSessionState::Starting { .. }) {
        return;
    }

    let mut all_connected = true;
    for index in 0..session.remote_peers.len() {
        let entity = session.remote_peers[index].link;
        let Ok((connected, sender, receiver)) = links.get_mut(entity) else {
            all_connected = false;
            continue;
        };
        if !connected || sender.is_none() {
            all_connected = false;
            continue;
        }
        // Receiver insertion requested by P2PStart can still be deferred for this frame. It does
        // not prevent us from sending our own Ready message once every Link is connected.
        let Some(mut receiver) = receiver else {
            continue;
        };
        for message in receiver.receive() {
            tracing::trace!(?entity, ?message, tick = ?timeline.tick(), "received P2P session message");
            session.receive(entity, message);
        }
    }

    if !all_connected {
        return;
    }
    match session.advance(timeline.tick(), synced_timeline.is_some()) {
        AdvanceResult::Started(start_tick) => {
            tracing::info!(?start_tick, "P2P session started");
            commands.trigger(P2PStarted { start_tick });
        }
        AdvanceResult::Failed => {
            tracing::warn!("P2P session start negotiation failed");
        }
        AdvanceResult::Waiting => {}
    }

    let mut outbound = SmallVec::<[(Entity, P2PSessionMessage); 8]>::new();
    session.collect_outbound(&mut outbound);
    for (entity, message) in outbound {
        let Ok((_, Some(mut sender), _)) = links.get_mut(entity) else {
            continue;
        };
        tracing::trace!(?entity, ?message, tick = ?timeline.tick(), "sending P2P session message");
        sender.send::<P2PSessionChannel>(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).unwrap()
    }

    #[test]
    fn start_freezes_declared_links_without_requiring_the_capacity() {
        let mut app = App::new();
        app.insert_resource(P2PSession::new(4));
        app.add_observer(start_session);
        let second = app.world_mut().spawn(P2P).id();
        let first = app.world_mut().spawn(P2P).id();

        app.world_mut().trigger(P2PStart);
        app.world_mut().flush();
        assert!(
            app.world()
                .entity(first)
                .contains::<MessageReceiver<P2PSessionMessage>>()
        );
        {
            let session = app.world().resource::<P2PSession>();
            assert_eq!(
                session.state(),
                P2PSessionState::Starting { start_tick: None }
            );
            let links: SmallVec<[Entity; 4]> = session.links().collect();
            assert_eq!(links.len(), 2);
            assert!(links.contains(&first));
            assert!(links.contains(&second));
        }

        let late = app.world_mut().spawn(P2P).id();
        assert!(
            !app.world()
                .resource::<P2PSession>()
                .links()
                .any(|link| link == late)
        );
    }

    #[test]
    fn ready_peers_choose_and_acknowledge_the_latest_tick() {
        let mut session = P2PSession::new(3).with_start_delay_ticks(10);
        let first = entity(1);
        let second = entity(2);
        session.begin(SmallVec::from_slice(&[first, second]));

        assert_eq!(session.advance(Tick(5), false), AdvanceResult::Waiting);
        assert_eq!(session.advance(Tick(6), true), AdvanceResult::Waiting);
        session.receive(
            first,
            P2PSessionMessage::Ready {
                generation: session.generation,
                earliest_start_tick: Tick(18),
            },
        );
        session.receive(
            second,
            P2PSessionMessage::Ready {
                generation: session.generation,
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

    #[derive(Resource, Default)]
    struct WasStopped(bool);

    fn record_stopped(_trigger: On<P2PStopped>, mut stopped: ResMut<WasStopped>) {
        stopped.0 = true;
    }

    #[test]
    fn stop_keeps_links_and_emits_stopped() {
        let mut app = App::new();
        app.add_plugins(lightyear_link::LinkPlugin);
        let link = app.world_mut().spawn(P2P).id();
        let mut session = P2PSession::new(2);
        session.begin(SmallVec::from_slice(&[link]));
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
        let cohort_link = app.world_mut().spawn(P2P).id();
        let late_link = app.world_mut().spawn(P2P).id();
        let mut session = P2PSession::new(2);
        session.begin(SmallVec::from_slice(&[cohort_link]));
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
