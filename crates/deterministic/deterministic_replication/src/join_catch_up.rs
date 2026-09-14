//! Catching a joining peer up by replaying the session's inputs.
//!
//! Unlike [`crate::late_join`], this path transfers no world-state snapshot. Every peer records
//! session inputs; the newcomer rebuilds the initial world and runs those inputs through the
//! application's fixed schedule. The checksum uses the application's registered deterministic
//! components, but no peer becomes a state-replication authority.
//!
//! # Why inputs rather than snapshots
//!
//! In input-only deterministic simulation nobody owns the world: each peer derives it from the same
//! initial state plus the same inputs. A newcomer is missing exactly two things — the initial world,
//! which its own application builds, and the inputs. Handing it the inputs is enough for it to arrive
//! at the same state, with no component transfer and no peer acting as an authority.
//!
//! # Replay and live handoff
//!
//! The initial replay rewinds `LocalTimeline` and fixed time **before** triggering
//! [`P2PCatchUpReplay`], then runs `FixedMain` forward from the founding session's first tick.
//! This is not an ordinary rollback: the newcomer has no historical world to restore.
//! Earlier player activations are replayed at their original ticks.
//!
//! The application must create remote input targets on [`P2PJoinCatchUp`] and rebuild its
//! initial world on [`P2PCatchUpReplay`]. The newcomer's own player must remain absent until
//! `P2PJoined`; historical players must not carry the local input marker.
//!
//! ```text
//! newcomer                                  donor
//!   |-- CatchUpRequest ---------------------->|
//!   |<-- CatchUpInputs + CatchUpEnd ----------|  complete, request-tagged transfer
//!   |                                        |
//!   | rebuild once                           |
//!   | replay a bounded batch each frame      |
//!   | verify the initial target checksum     |
//!   | keep receiving and retaining live input|
//!   | reach the all-remote-input frontier    |
//!   | P2PJoinCatchUpComplete                  |
//! ```
//!
//! Networking continues between replay batches. Receive-only buffers keep live inputs separate
//! from the historical simulation buffers, including while the reliable transfer is pending.
//! The replay chases the current all-remote-input frontier without predicting past it; no second
//! reliable tail transfer is required. Transfers complete by record count, not end-message order.
//!
//! # Cost
//!
//! Recording costs one entry per player per tick; replay work is linear in elapsed session ticks.
//! [`JoinCatchUpConfig::max_history_ticks`] bounds the archive. A session beyond that bound cannot
//! be joined through this path: truncated history is refused, not replayed against an invented
//! initial state. [`JoinCatchUpConfig::max_catchup_ticks_per_frame`] defaults to 10 and bounds
//! simulation work per frame, not transfer decoding or the application's world rebuild.

use alloc::vec::Vec;
use bevy_app::{
    App, FixedMain, FixedPreUpdate, Plugin, PostUpdate, PreUpdate, RunFixedMainLoop,
    RunFixedMainLoopSystems,
};
use bevy_ecs::prelude::*;
use bevy_platform::collections::HashMap;
use bevy_time::{Fixed, Time};
use core::marker::PhantomData;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_core::tick::TickDuration;
use lightyear_inputs::input_buffer::InputBuffer;
use lightyear_inputs::input_message::{ActionStateSequence, Compressed};
use lightyear_inputs::prelude::RemoteInputTarget;
use lightyear_inputs::prelude::client::InputSystems;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageReceiver, MessageSender};
use lightyear_p2p::prelude::P2PSession;
use lightyear_p2p::{
    JoinRejectReason, P2PChannel, P2PJoinCancel, P2PJoinCatchUp, P2PJoinCatchUpComplete,
    P2PJoinPlugin,
};
#[cfg(test)]
use lightyear_prediction::prelude::{PredictionManager, StateRollbackMetadata};
use lightyear_replication::prelude::PreSpawned;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// Tunables for input-replay catch-up.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct JoinCatchUpConfig {
    /// Whether peers record their inputs so that they can catch a newcomer up.
    ///
    /// Recording costs one entry per player per tick for the whole session, so an application that
    /// never admits a joiner leaves this off.
    pub enabled: bool,
    /// How many ticks of history a peer keeps.
    ///
    /// A catch-up is only possible from the start of the session, so this bounds the session length
    /// that can still be joined. It is a hard bound rather than a sliding window: replaying from a
    /// truncated history would start the newcomer at a tick whose world it does not have.
    pub max_history_ticks: u32,
    /// How many records are sent per message.
    ///
    /// A long session produces a lot of records: one message per record would be far more messages
    /// than the transport needs, and one message per session would exceed a packet.
    pub records_per_message: u32,
    /// Maximum historical simulation ticks executed per rendered frame. Must be nonzero.
    pub max_catchup_ticks_per_frame: u32,
}

impl Default for JoinCatchUpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            // One hour at 60 Hz. The bound is also the memory bound: recording stops once the
            // session outgrows it, because a history that no longer reaches the session's start
            // cannot replay it. A shorter session only ever uses what it needs.
            max_history_ticks: 60 * 60 * 60,
            records_per_message: 256,
            max_catchup_ticks_per_frame: 10,
        }
    }
}

/// One player's inputs for one tick, in the form the wire carries them.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedInput<S> {
    /// Stable identity of the player, matching the `PreSpawned` hash every peer keys its input
    /// entities on.
    hash: u64,
    /// The tick these inputs belong to.
    tick: Tick,
    /// The inputs themselves, in the sequence form the input pipeline already round-trips.
    sequence: S,
}

/// Every input this peer has seen since the session started.
///
/// The history is what a newcomer needs in order to reconstruct the session, so it must be complete:
/// it starts at the session's first gameplay tick and never drops an entry from the middle.
#[derive(Resource, Debug)]
pub struct InputHistory<S> {
    records: Vec<RecordedInput<S>>,
    joins: Vec<SessionJoin>,
    /// Earliest tick the history covers, once recording has begun.
    ///
    /// A minimum rather than the first tick seen: players' inputs arrive independently, so one
    /// player's later tick can be recorded before another player's earlier one.
    start_tick: Option<Tick>,
    /// Latest tick the history covers.
    ///
    /// The span between this and [`start_tick`](Self::start_tick) is what the capacity bound
    /// measures, because the span is what a replay has to cover.
    end_tick: Option<Tick>,
    /// Latest tick recorded per player.
    ///
    /// Recording is per player, and one player's inputs can arrive ahead of another's, so this is
    /// what keeps a late arrival from being recorded twice.
    recorded_until: HashMap<u64, Tick>,
    /// Set when the session outgrew [`JoinCatchUpConfig::max_history_ticks`].
    truncated: bool,
    /// The tick the session started playing at.
    ///
    /// Recording is anchored here rather than at the first tick that happened to have data. A
    /// history that begins after the session cannot replay it, so the anchor is what makes "does
    /// this history reach the session's start?" answerable.
    session_start_tick: Option<Tick>,
}

impl<S> Default for InputHistory<S> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            joins: Vec::new(),
            start_tick: None,
            end_tick: None,
            recorded_until: HashMap::default(),
            truncated: false,
            session_start_tick: None,
        }
    }
}

impl<S> InputHistory<S> {
    /// The first tick the history covers, once recording has begun.
    pub fn start_tick(&self) -> Option<Tick> {
        self.start_tick
    }

    /// Anchor the history at the tick the session started playing at.
    pub fn set_session_start(&mut self, tick: Tick) {
        self.session_start_tick = Some(tick);
    }

    /// The next tick this player's inputs are still missing.
    fn next_missing(&self, hash: u64) -> Option<Tick> {
        self.recorded_until.get(&hash).map(|last| *last + 1)
    }

    /// How many tick entries the history holds.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Whether the session outgrew the configured history bound.
    ///
    /// A truncated history cannot replay a session from its start, so this peer cannot catch anyone
    /// up. It says so rather than sending a history that would produce a different world.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Record one player's inputs for one tick.
    ///
    /// Returns `false` when the entry was already recorded, which happens when a reliable
    /// retransmission delivers the same inputs again.
    fn record(&mut self, hash: u64, tick: Tick, sequence: S, max_history_ticks: u32) -> bool {
        if let Some(recorded_until) = self.recorded_until.get(&hash)
            && *recorded_until >= tick
        {
            return false;
        }
        // The span is what a replay covers, and inputs can arrive slightly out of order, so both
        // ends are tracked rather than assuming the first tick seen is the earliest.
        let start_tick = self.start_tick.map_or(tick, |start| start.min(tick));
        let end_tick = self.end_tick.map_or(tick, |end| end.max(tick));
        // A session that outgrew the bound can no longer be replayed from its beginnings, so the
        // history is marked unusable instead of being allowed to grow without limit.
        if (end_tick - start_tick) as u32 > max_history_ticks {
            if !self.truncated {
                tracing::warn!(
                    max_history_ticks,
                    "the P2P session outgrew the join history bound; it can no longer be replayed \
                     from its start and cannot catch a peer up"
                );
            }
            self.truncated = true;
            return false;
        }
        self.start_tick = Some(start_tick);
        self.end_tick = Some(end_tick);
        self.recorded_until.insert(hash, tick);
        self.records.push(RecordedInput {
            hash,
            tick,
            sequence,
        });
        true
    }
}
/// Membership changes are simulation events too; later joiners must replay earlier activations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SessionJoin {
    peer: PeerId,
    tick: Tick,
}

/// Ask a peer for every input since the session started.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct CatchUpRequest {
    id: u64,
}

/// A chunk of the session's inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatchUpInputs<S> {
    id: u64,
    records: Vec<RecordedInput<S>>,
}

/// The end of a catch-up transfer, or a refusal to serve one.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatchUpEnd {
    id: u64,
    joins: Vec<SessionJoin>,
    /// First tick the history covers, which is where the replay starts.
    ///
    /// `None` when this donor had nothing to replay, which the joiner treats as a refusal.
    start_tick: Option<Tick>,
    /// Tick the newcomer must replay up to.
    target_tick: Option<Tick>,
    /// The tick the session started playing at, as this donor records it.
    session_start_tick: Option<Tick>,
    /// What this donor's world hashed to at [`Self::target_tick`].
    ///
    /// The joiner recomputes this after its replay. Agreeing on the inputs and the tick is not the
    /// same as agreeing on the world: a replay that is subtly wrong produces the right shape of
    /// state, and nothing else in the exchange would notice.
    checksum: Option<u64>,
    /// Number of records in this transfer. The reliable channel may deliver this before its chunks.
    record_count: usize,
    /// Why the transfer was refused.
    refusal: Option<JoinRejectReason>,
}

/// Registers the catch-up transfer's messages on the shared P2P control channel.
///
/// Installed with [`JoinCatchUpPlugin`]; every peer that may serve a catch-up exchange has to reserve
/// the same ids.
#[doc(hidden)]
pub struct JoinCatchUpProtocolPlugin<S> {
    sequence: PhantomData<S>,
}

impl<S> Default for JoinCatchUpProtocolPlugin<S> {
    fn default() -> Self {
        Self {
            sequence: PhantomData,
        }
    }
}

impl<S: ActionStateSequence> Plugin for JoinCatchUpProtocolPlugin<S> {
    fn build(&self, app: &mut App) {
        app.register_message::<CatchUpRequest>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<CatchUpInputs<S>>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<CatchUpEnd>()
            .add_direction(NetworkDirection::Bidirectional);
    }
}

/// What a joining peer has collected from the donor.
#[derive(Resource, Debug)]
struct CatchUpTransfer<S> {
    id: u64,
    donor: Option<Entity>,
    records: Vec<RecordedInput<S>>,
    end: Option<CatchUpEnd>,
}

impl<S> Default for CatchUpTransfer<S> {
    fn default() -> Self {
        Self {
            id: 0,
            donor: None,
            records: Vec::new(),
            end: None,
        }
    }
}

impl<S> CatchUpTransfer<S> {
    fn take_complete(&mut self) -> Option<(Vec<RecordedInput<S>>, CatchUpEnd)> {
        let end = self.end.as_ref()?;
        if self.records.len() != end.record_count {
            return None;
        }
        let end = self.end.take().unwrap();
        let mut records = core::mem::take(&mut self.records);
        // The archive is append-only per player, whereas chunks arrive reliably but unordered.
        records.sort_unstable_by_key(|record| (record.tick, record.hash));
        Some((records, end))
    }
}

/// Installs input-replay catch-up for a deterministic P2P session.
///
/// Recording runs on every peer; the transfer serves a newcomer that asks for the session's inputs.
/// It stays inert until an application enables it, because recording costs memory for the whole
/// session.
pub struct JoinCatchUpPlugin<S> {
    config: JoinCatchUpConfig,
    sequence: PhantomData<S>,
}

impl<S> JoinCatchUpPlugin<S> {
    /// Install input-replay catch-up with an explicit configuration.
    ///
    /// Use this rather than [`Default`] to enable recording: it is off by default, because it costs
    /// an entry per player per tick for as long as the session runs.
    pub fn new(config: JoinCatchUpConfig) -> Self {
        Self {
            config,
            sequence: PhantomData,
        }
    }

    /// Enable recording with everything else left at its defaults.
    ///
    /// This is the common case: an application that wants its sessions to be joinable says so, and
    /// takes the default history bound.
    pub fn enabled() -> Self {
        Self::new(JoinCatchUpConfig {
            enabled: true,
            ..Default::default()
        })
    }

    pub fn config(&self) -> JoinCatchUpConfig {
        self.config
    }
}

impl<S> Default for JoinCatchUpPlugin<S> {
    fn default() -> Self {
        Self {
            config: JoinCatchUpConfig::default(),
            sequence: PhantomData,
        }
    }
}

impl<S: ActionStateSequence> Plugin for JoinCatchUpPlugin<S> {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<P2PJoinPlugin>() {
            app.add_plugins(P2PJoinPlugin);
        }
        if !app.is_plugin_added::<JoinCatchUpProtocolPlugin<S>>() {
            app.add_plugins(JoinCatchUpProtocolPlugin::<S>::default());
        }
        // The plugin's configuration is the resource, unless the application already inserted one,
        // which keeps it free to configure catch-up without going through the plugin.
        if !app.world().contains_resource::<JoinCatchUpConfig>() {
            app.insert_resource(self.config);
        }
        app.init_resource::<InputHistory<S>>();
        app.init_resource::<CatchUpTransfer<S>>();
        app.init_resource::<CatchUpReplayInputs<S>>();

        // Anchor the history at the session's first gameplay tick, on both paths that start a
        // session: founding it, and joining one.
        app.add_observer(anchor_history_on_start::<S>);
        app.add_observer(request_catch_up::<S>);
        app.add_observer(record_session_join::<S>);
        app.add_observer(reset_transfer::<S, P2PJoinCancel>);
        app.add_observer(reset_transfer::<S, lightyear_p2p::P2PStopped>);
        // Every replay tick needs its inputs before the input pipeline reads them, so feed them
        // inside the fixed step rather than once per frame.
        app.add_systems(
            FixedPreUpdate,
            stream_replay_inputs::<S>.before(InputSystems::BufferClientInputs),
        );
        app.configure_sets(
            PreUpdate,
            lightyear_prediction::plugin::PredictionSystems::Rollback.run_if(not_replaying),
        );
        app.configure_sets(
            RunFixedMainLoop,
            RunFixedMainLoopSystems::FixedMainLoop.run_if(not_replaying),
        );
        app.add_systems(
            PreUpdate,
            (capture_live_inputs::<S>, receive_catch_up::<S>)
                .chain()
                .after(lightyear_prediction::plugin::PredictionSystems::Rollback)
                .after(InputSystems::ReceiveInputMessages),
        );
        app.add_systems(
            RunFixedMainLoop,
            (run_catch_up_replay::<S>, finish_catch_up_replay::<S>)
                .chain()
                .after(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .before(RunFixedMainLoopSystems::FixedMainLoop),
        );

        // Recording reads the input buffers, so it has to run after every input type has written
        // them for this frame.
        app.add_systems(
            PostUpdate,
            record_input_history::<S>
                .run_if(recording_enabled)
                .after(InputSystems::UpdateRemoteInputTicks)
                .before(InputSystems::CleanUp),
        );
        // The checksum must describe corrected state, not the prediction from before input receipt.
        app.add_systems(
            PostUpdate,
            serve_catch_up_request::<S>
                .after(record_input_history::<S>)
                .before(MessageSystems::Send),
        );
    }
}

fn recording_enabled(config: Res<JoinCatchUpConfig>) -> bool {
    config.enabled
}

fn not_replaying(replay: Option<Res<CatchUpReplay>>) -> bool {
    replay.is_none()
}

/// The tick a replay may run up to: the latest tick for which **every** player's inputs are known.
///
/// Replaying further would simulate ticks with a player's inputs missing, and the newcomer would end
/// up disagreeing with the session it is joining. This is the same frontier the prediction window
/// uses, computed from the buffers rather than from the local tick.
fn replay_target(frontiers: impl IntoIterator<Item = Tick>) -> Option<Tick> {
    frontiers.into_iter().reduce(Tick::min)
}

/// Append this tick's inputs from every player to the history.
///
/// One entry per player per tick is what lets a newcomer re-run the session exactly: a player whose
/// inputs have not arrived yet is simply not recorded for that tick, and the tick range of the
/// history then shows the replay how far it can safely go.
fn record_input_history<S: ActionStateSequence>(
    config: Res<JoinCatchUpConfig>,
    timeline: Res<LocalTimeline>,
    mut history: ResMut<InputHistory<S>>,
    transfer: Option<Res<CatchUpTransfer<S>>>,
    replay: Option<Res<CatchUpReplay>>,
    players: Query<(
        &PreSpawned,
        &InputBuffer<S::Snapshot, S::Action>,
        Has<S::Marker>,
    )>,
) {
    if replay.is_some()
        || transfer
            .as_ref()
            .is_some_and(|transfer| transfer.donor.is_some())
    {
        return;
    }
    let tick = timeline.tick();
    let max_history_ticks = config.max_history_ticks;
    // Without an anchor there is no session to be caught up to, and nothing to anchor the span
    // against.
    let Some(session_start) = history.session_start_tick else {
        return;
    };

    for (pre_spawned, buffer, local) in &players {
        let Some(hash) = pre_spawned.hash else {
            continue;
        };
        // Only ticks this player's inputs are actually known for can be recorded.
        //
        // A remote player's buffer predicts past the last input it received, holding the previous
        // value forward, and reports nothing before its window begins. Recording a tick from either
        // region would store a value the session never had, and the history is append-only: a tick
        // recorded wrongly can never be corrected. So a remote player is recorded up to the frontier
        // it has actually received, and a local player — whose inputs this peer originates — up to
        // the tick it is on.
        //
        // Advancing one tick at a time from the anchor is what keeps the history contiguous from the
        // session's start: the early ticks arrive a few frames late for a remote player, and waiting
        // for them here fills them in rather than skipping them.
        let Some(confirmed_until) = (if local {
            Some(tick)
        } else {
            buffer.last_remote_tick
        }) else {
            continue;
        };
        let confirmed_until = confirmed_until.min(tick);
        // New players begin at their own activation, not the founding session's first tick.
        let first_tick = buffer
            .start_tick
            .unwrap_or(session_start)
            .max(session_start);
        let mut next = history
            .next_missing(hash)
            .unwrap_or(first_tick)
            .max(session_start);
        while next <= confirmed_until {
            // A sequence covering exactly this tick, built by the same helper the input pipeline uses
            // to send it over the wire.
            if buffer.get(next).is_none() {
                break;
            }
            let Some(sequence) = S::build_from_input_buffer(buffer, 1, next) else {
                break;
            };
            if !history.record(hash, next, sequence, max_history_ticks) {
                break;
            }
            next += 1;
        }
    }
}

/// Anchor the input history at the tick the session started playing at.
///
/// A session begins at the tick the world is built for; the inputs for that tick arrive during and
/// after it, so recording anchored at the first tick that happens to have data would begin late.
fn anchor_history_on_start<S: ActionStateSequence>(
    trigger: On<lightyear_p2p::prelude::P2PStarted>,
    mut history: ResMut<InputHistory<S>>,
) {
    *history = InputHistory::default();
    history.set_session_start(trigger.start_tick);
}

fn record_session_join<S: ActionStateSequence>(
    trigger: On<lightyear_p2p::P2PJoined>,
    replay: Option<Res<CatchUpReplay>>,
    mut history: ResMut<InputHistory<S>>,
) {
    if replay.is_none() {
        history.joins.push(SessionJoin {
            peer: trigger.peer_id,
            tick: trigger.activate_tick,
        });
    }
}

fn reset_transfer<S: ActionStateSequence, E: Event>(_trigger: On<E>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        release_live_targets::<S>(world, true);
        if let Some(mut transfer) = world.get_resource_mut::<CatchUpTransfer<S>>() {
            transfer.donor = None;
            transfer.records.clear();
            transfer.end = None;
        }
        if let Some(mut inputs) = world.get_resource_mut::<CatchUpReplayInputs<S>>() {
            inputs.by_tick.clear();
        }
        world.remove_resource::<CatchUpReplay>();
    });
}

/// Identifies a receive-only buffer, without introducing a second replication identity.
#[derive(Component)]
struct CatchUpLiveBuffer {
    hash: u64,
}

/// Archive new confirmed samples before the 64-slot live ring can evict them.
fn capture_live_inputs<S: ActionStateSequence>(
    config: Res<JoinCatchUpConfig>,
    transfer: Option<Res<CatchUpTransfer<S>>>,
    replay: Option<Res<CatchUpReplay>>,
    mut history: ResMut<InputHistory<S>>,
    mut inputs: ResMut<CatchUpReplayInputs<S>>,
    players: Query<(
        Option<&PreSpawned>,
        Option<&CatchUpLiveBuffer>,
        &InputBuffer<S::Snapshot, S::Action>,
    )>,
) {
    if replay.is_none()
        && transfer
            .as_ref()
            .is_none_or(|transfer| transfer.donor.is_none())
    {
        return;
    }
    let separated = replay
        .as_ref()
        .is_some_and(|replay| replay.replayed_to.is_some());
    for (target, live, buffer) in &players {
        if separated != live.is_some() {
            continue;
        }
        let Some(hash) = live
            .map(|live| live.hash)
            .or_else(|| target.and_then(|target| target.hash))
        else {
            continue;
        };
        let (Some(start), Some(end)) = (buffer.start_tick, buffer.last_remote_tick) else {
            continue;
        };
        let mut tick = history.next_missing(hash).unwrap_or(start);
        while tick <= end {
            let Some(snapshot) = buffer.get(tick) else {
                break;
            };
            let Some(sequence) = S::build_from_input_buffer(buffer, 1, tick) else {
                break;
            };
            if !history.record(hash, tick, sequence, config.max_history_ticks) {
                break;
            }
            inputs
                .by_tick
                .entry(tick)
                .or_default()
                .push((hash, snapshot.clone()));
            tick += 1;
        }
    }
}

/// Write the current tick's session inputs into the players' buffers while a replay runs.
///
/// The buffers are fixed-window rings, so a whole session's history cannot live in them at once. The
/// replay advances one tick at a time, so the inputs it needs are exactly the ones for the tick it is
/// about to simulate — which is what this writes, one tick per fixed step.
///
/// This has to run before the input pipeline reads the buffers in the same fixed step, or the step
/// simulates a tick with no input.
fn stream_replay_inputs<S: ActionStateSequence>(
    mut replay_inputs: ResMut<CatchUpReplayInputs<S>>,
    replay: Option<Res<CatchUpReplay>>,
    timeline: Res<LocalTimeline>,
    mut buffers: Query<(&PreSpawned, &mut InputBuffer<S::Snapshot, S::Action>)>,
) {
    if replay.is_none() {
        return;
    }
    let tick = timeline.tick();
    let Some(per_player) = replay_inputs.by_tick.remove(&tick) else {
        return;
    };
    for (pre_spawned, mut buffer) in &mut buffers {
        let Some(hash) = pre_spawned.hash else {
            continue;
        };
        let Some((_, snapshot)) = per_player.iter().find(|(recorded, _)| *recorded == hash) else {
            continue;
        };
        // A replay can precede the live buffer's retained window. Future samples are staged
        // separately, so resetting that window does not discard the live handoff.
        if buffer.start_tick.is_none_or(|start_tick| start_tick > tick) {
            *buffer = InputBuffer::default();
        }
        buffer.set_raw(tick, Some(snapshot.clone()));
        // Streaming historical input must not move a newer live frontier backwards.
        buffer.last_remote_tick = Some(buffer.last_remote_tick.map_or(tick, |old| old.max(tick)));
    }
}

/// Ask the session for the inputs it has seen.
///
/// Reuse the admission coordinator as the donor, including when it previously joined itself.
fn request_catch_up<S: ActionStateSequence>(
    trigger: On<P2PJoinCatchUp>,
    mut commands: Commands,
    config: Res<JoinCatchUpConfig>,
    metadata: Res<lightyear_connection::network_topology::NetworkingMetadata>,
    roster: Query<&P2P, With<lightyear_connection::client::Connected>>,
    mut transfer: ResMut<CatchUpTransfer<S>>,
    mut history: ResMut<InputHistory<S>>,
    mut inputs: ResMut<CatchUpReplayInputs<S>>,
    mut senders: Query<&mut MessageSender<CatchUpRequest>>,
) {
    if !config.enabled {
        // Recording is what makes a catch-up possible; without it the attempt cannot succeed.
        tracing::error!("cannot catch this peer up because JoinCatchUpConfig::enabled is false");
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::CatchUpFailed,
        });
        return;
    }
    let Some(donor) = metadata
        .peer_map
        .get(&trigger.bootstrap)
        .copied()
        .filter(|link| {
            roster
                .get(*link)
                .is_ok_and(|state| *state == P2P::Candidate)
        })
    else {
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::RosterIncomplete,
        });
        return;
    };
    let Ok(mut sender) = senders.get_mut(donor) else {
        tracing::error!(
            ?donor,
            "the donor Link cannot carry a catch-up request; it has no sender for the message"
        );
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::CatchUpFailed,
        });
        return;
    };
    transfer.id = transfer.id.wrapping_add(1);
    transfer.donor = Some(donor);
    transfer.records.clear();
    transfer.end = None;
    *history = InputHistory::default();
    inputs.by_tick.clear();
    history.session_start_tick = trigger.session_start_tick;
    sender.send::<P2PChannel>(CatchUpRequest { id: transfer.id });
    tracing::info!(peer = ?trigger.bootstrap, ?donor, "asked the admission coordinator for its inputs");
}

/// Answer a newcomer that asked for the session's inputs.
///
/// The donor sends the whole history and then the two ticks. It refuses when the session outgrew the
/// history bound: a truncated history would replay from a tick whose world the newcomer does not
/// have, which would leave it quietly disagreeing with everyone.
fn serve_catch_up_request<S: ActionStateSequence>(
    session: Option<Res<P2PSession>>,
    mut world: crate::archetypes::ChecksumWorld<'_, '_, true>,
    config: Res<JoinCatchUpConfig>,
    history: Res<InputHistory<S>>,
    timeline: Res<LocalTimeline>,
    players: Query<(&PreSpawned, &InputBuffer<S::Snapshot, S::Action>)>,
    mut links: Query<(
        Option<&mut MessageReceiver<CatchUpRequest>>,
        Option<&mut MessageSender<CatchUpInputs<S>>>,
        Option<&mut MessageSender<CatchUpEnd>>,
    )>,
) {
    let records_per_message = config.records_per_message.max(1) as usize;
    // Read once: the session's start tick is the same for every Link this peer serves.
    let session_start_tick = history.session_start_tick;
    for (receiver, inputs, end) in &mut links {
        let (Some(mut receiver), Some(mut inputs), Some(mut end)) = (receiver, inputs, end) else {
            continue;
        };
        for request in receiver.receive() {
            if !config.enabled
                || history.is_truncated()
                || session.as_ref().is_none_or(|s| !s.is_started())
            {
                end.send::<P2PChannel>(CatchUpEnd {
                    id: request.id,
                    start_tick: None,
                    target_tick: None,
                    session_start_tick: None,
                    checksum: None,
                    refusal: Some(JoinRejectReason::CatchUpFailed),
                    record_count: 0,
                    joins: Vec::new(),
                });
                continue;
            }
            let target = players
                .iter()
                .map(|(target, _)| {
                    target
                        .hash
                        .and_then(|hash| history.recorded_until.get(&hash).copied())
                })
                .collect::<Option<SmallVec<[Tick; 4]>>>()
                .and_then(replay_target)
                .map(|tick| tick.min(timeline.tick()));
            let mut records = history.records.iter().peekable();
            let mut record_count = 0;
            while records.peek().is_some() {
                let chunk: Vec<_> = records
                    .by_ref()
                    .take(records_per_message)
                    .cloned()
                    .collect();
                record_count += chunk.len();
                inputs.send::<P2PChannel>(CatchUpInputs {
                    id: request.id,
                    records: chunk,
                });
            }
            tracing::info!(
                records = record_count,
                start_tick = ?history.start_tick(),
                ?target,
                "served a P2P join catch-up"
            );
            // Hash this peer's world at the tick the joiner will replay to. The joiner recomputes
            // it after its replay and refuses the catch-up if the two disagree.
            let checksum = target.and_then(|target_tick| {
                world.update_archetypes();
                let (checksum, coverage) = crate::checksum::compute_history_checksum_with_coverage(
                    &mut world,
                    target_tick,
                );
                tracing::info!(
                    ?target_tick,
                    checksum = format_args!("{checksum:016x}"),
                    entries = coverage.entries,
                    entities = coverage.entities,
                    per_component = ?coverage.summary(),
                    "checksum coverage, donor side"
                );
                (coverage.entries > 0).then_some(checksum)
            });
            end.send::<P2PChannel>(CatchUpEnd {
                id: request.id,
                joins: history.joins.clone(),
                start_tick: history.start_tick(),
                target_tick: target,
                session_start_tick,
                checksum,
                record_count,
                refusal: None,
            });
        }
    }
}

/// Collect a catch-up transfer, then replay it.
///
/// The replay is requested in the frame the transfer completes, so the joiner goes from "world
/// built, nothing simulated" to "rewinding" without simulating a tick of its own in between.
#[allow(clippy::too_many_arguments)]
fn receive_catch_up<S: ActionStateSequence>(
    mut commands: Commands,
    config: Res<JoinCatchUpConfig>,
    tick_duration: Res<TickDuration>,
    timeline: Res<LocalTimeline>,
    mut transfer: ResMut<CatchUpTransfer<S>>,
    mut history: ResMut<InputHistory<S>>,
    mut replay_inputs: ResMut<CatchUpReplayInputs<S>>,
    mut links: Query<(
        Entity,
        Option<&mut MessageReceiver<CatchUpInputs<S>>>,
        Option<&mut MessageReceiver<CatchUpEnd>>,
    )>,
) {
    for (entity, chunks, end) in &mut links {
        if let Some(mut chunks) = chunks {
            for chunk in chunks.receive() {
                if transfer.donor == Some(entity) && chunk.id == transfer.id {
                    transfer.records.extend(chunk.records);
                }
            }
        }
        let Some(mut end) = end else {
            continue;
        };
        for message in end.receive() {
            if transfer.donor != Some(entity) || message.id != transfer.id {
                continue;
            }
            if let Some(refusal) = message.refusal {
                tracing::warn!(?refusal, "the session refused to serve a catch-up");
                transfer.donor = None;
                transfer.records.clear();
                transfer.end = None;
                commands.trigger(P2PJoinCancel { reason: refusal });
                return;
            }
            transfer.end = Some(message);
        }
    }
    let Some((records, end)) = transfer.take_complete() else {
        return;
    };

    if end.checksum.is_none() {
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::CatchUpFailed,
        });
        return;
    }
    history.joins = end.joins;
    apply_catch_up::<S>(
        &mut commands,
        timeline.tick(),
        tick_duration.0,
        records,
        end.start_tick,
        end.target_tick,
        end.session_start_tick,
        end.checksum,
        &mut history,
        &mut replay_inputs,
    );
}

/// Seed a complete catch-up into the input buffers and request the replay.
///
/// Separate from [`receive_catch_up`] so the replay can be tested without a transport: the test
/// supplies the records and the two ticks directly, which is the same shape the donor sends.
#[allow(clippy::too_many_arguments)]
fn apply_catch_up<S: ActionStateSequence>(
    commands: &mut Commands,
    current_tick: Tick,
    tick_duration: core::time::Duration,
    records: Vec<RecordedInput<S>>,
    start_tick: Option<Tick>,
    target_tick: Option<Tick>,
    session_start_tick: Option<Tick>,
    donor_checksum: Option<u64>,
    history: &mut InputHistory<S>,
    inputs: &mut CatchUpReplayInputs<S>,
) {
    let (Some(start_tick), Some(target_tick)) = (start_tick, target_tick) else {
        // The donor had no inputs to send, so there is nothing to replay.
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::CatchUpFailed,
        });
        return;
    };

    // The replay runs from the world this peer just built, which is the session at its first
    // gameplay tick. A history that begins after that tick cannot reproduce it: the ticks in
    // between would be simulated from inputs nobody recorded, and the two peers would diverge
    // silently. So this is refused, loudly, rather than replayed into a world that only looks right.
    match session_start_tick {
        Some(session_start_tick) if start_tick <= session_start_tick => {}
        Some(session_start_tick) => {
            tracing::error!(
                ?start_tick,
                ?session_start_tick,
                "the session's inputs begin after the session did; this peer cannot be caught up \
                 from them"
            );
            commands.trigger(P2PJoinCancel {
                reason: JoinRejectReason::CatchUpFailed,
            });
            return;
        }
        None => {
            tracing::error!(
                "the donor did not report the tick the session started at, so a replay from this \
                 history cannot be verified; refusing the catch-up"
            );
            commands.trigger(P2PJoinCancel {
                reason: JoinRejectReason::CatchUpFailed,
            });
            return;
        }
    }
    let start_tick = session_start_tick.unwrap();

    // Decode the transfer into per-tick snapshots, which is what the replay streams into the
    // buffers. A one-tick sequence carries exactly one snapshot.
    let by_tick = &mut inputs.by_tick;
    for record in &records {
        let Some(snapshot) = record
            .sequence
            .clone()
            .get_snapshots_from_message(tick_duration)
            .next()
            .and_then(|compressed| match compressed {
                Compressed::Input(snapshot) => Some(snapshot),
                // A tick with no input is a tick this peer must not invent one for.
                Compressed::Absent | Compressed::SameAsPrecedent => None,
            })
        else {
            continue;
        };
        let row = by_tick.entry(record.tick).or_default();
        if let Some((_, existing)) = row.iter_mut().find(|(hash, _)| *hash == record.hash) {
            *existing = snapshot;
        } else {
            row.push((record.hash, snapshot));
        }
    }
    if by_tick.is_empty() {
        tracing::error!("a catch-up arrived but carried no inputs to replay");
        commands.trigger(P2PJoinCancel {
            reason: JoinRejectReason::CatchUpFailed,
        });
        return;
    }
    let ticks = by_tick.len();

    let live = core::mem::take(history);
    history.set_session_start(start_tick);
    history.joins = live.joins;
    for record in records {
        history.record(record.hash, record.tick, record.sequence, u32::MAX);
    }
    for record in live.records {
        history.record(record.hash, record.tick, record.sequence, u32::MAX);
    }

    // The replay itself is a forward run of the fixed loop from here, not a rollback: see
    // [`run_catch_up_replay`]. All this has to do is leave it the tick to start at and the inputs to
    // feed it.
    commands.insert_resource(CatchUpReplay {
        start_tick,
        target_tick,
        donor_checksum,
        requested_at: bevy_platform::time::Instant::now(),
        replayed_to: None,
        local_checksum: None,
        fixed_time: None,
        complete: false,
        failed: false,
    });

    tracing::info!(
        ticks,
        ?start_tick,
        ?target_tick,
        current_tick = ?current_tick,
        "the session's inputs are in hand; replaying the session"
    );
}

/// The session's inputs for the replay to stream into the input buffers.
///
/// An [`InputBuffer`] holds a fixed window of ticks — it is a ring, sized for steady-state play and
/// deliberately not growable. A catch-up covers a whole session, which does not fit, and seeding the
/// history into the ring leaves only its tail: the replay then finds no input for the ticks it
/// visits and simulates nothing.
///
/// So the history is kept here instead, and one tick's inputs are written into the buffers as the
/// replay reaches that tick. The ring only ever holds the window the simulation is reading.
#[derive(Resource, Debug)]
struct CatchUpReplayInputs<S: ActionStateSequence> {
    /// Unconsumed historical ticks and the retained live suffix, keyed by player hash.
    by_tick: alloc::collections::BTreeMap<Tick, SmallVec<[(u64, S::Snapshot); 4]>>,
}

impl<S: ActionStateSequence> Default for CatchUpReplayInputs<S> {
    fn default() -> Self {
        Self {
            by_tick: alloc::collections::BTreeMap::new(),
        }
    }
}

/// The session's inputs are in hand and the replay is about to run.
///
/// The application **rebuilds the session's initial world**, not the current roster. Input targets
/// prepared on [`P2PJoinCatchUp`] have already been separated from the historical simulation.
/// This event fires once, after rewinding the timeline to the tick before the session started.
/// Historical player activations are replayed separately at their original gameplay ticks.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PCatchUpReplay {
    /// The tick the session started playing at, which the world must be built for.
    pub session_start_tick: Tick,
    /// The donor's confirmed tick, where the initial replay is checksummed.
    pub target_tick: Tick,
}

/// A catch-up replay that has been requested and not yet reported.
#[derive(Resource, Debug)]
struct CatchUpReplay {
    /// The session's first gameplay tick, which the replay simulates from.
    start_tick: Tick,
    /// Checksum boundary in the initial transfer; the live frontier can extend beyond it.
    target_tick: Tick,
    /// What the donor's world hashed to at `target_tick`, to check this replay against.
    donor_checksum: Option<u64>,
    /// Historical cursor, or `None` until the initial world has been rebuilt.
    replayed_to: Option<Tick>,
    local_checksum: Option<u64>,
    fixed_time: Option<Time<Fixed>>,
    complete: bool,
    failed: bool,
    requested_at: bevy_platform::time::Instant,
}

/// Replay the session by running the fixed loop forward from its first gameplay tick.
///
/// This is deliberately not a rollback. A rollback restores each component from its own history and
/// re-simulates from there; the peer being caught up has no such history — its world was built at
/// the session's start and has never simulated it — so there is nothing to restore from and the
/// only correct starting state is the world as the application just built it. Rewinding the timeline
/// and running [`FixedMain`] once per tick advances that world exactly as it advanced on every peer
/// that was already there, with each tick fed the inputs the session recorded for it.
///
/// Each call advances at most the configured frame budget and leaves the timeline at its cursor.
/// The private fixed clock resumes with the next batch; networking keeps its live clock.
fn run_catch_up_replay<S: ActionStateSequence>(world: &mut World) {
    let Some(replay) = world.get_resource::<CatchUpReplay>() else {
        return;
    };
    if replay.complete || replay.failed {
        return;
    }
    let start_tick = replay.start_tick;
    let target_tick = replay.target_tick;
    let initialize = replay.replayed_to.is_none();
    let verify = replay.donor_checksum.is_some();
    let budget = world
        .resource::<JoinCatchUpConfig>()
        .max_catchup_ticks_per_frame;
    if budget == 0 || target_tick < start_tick {
        tracing::error!(
            budget,
            ?start_tick,
            ?target_tick,
            "invalid catch-up replay configuration"
        );
        world.resource_mut::<CatchUpReplay>().failed = true;
        return;
    }
    let live_fixed = *world.resource::<Time<Fixed>>();
    let live_time = *world.resource::<Time>();
    let mut players = world.query::<(&PreSpawned, &mut InputBuffer<S::Snapshot, S::Action>)>();
    if initialize {
        // Move live rings to receive-only entities; historical entities may not exist yet.
        let live: Vec<_> = players
            .iter_mut(world)
            .filter_map(|(target, mut buffer)| {
                target
                    .hash
                    .map(|hash| (hash, target.receiver, core::mem::take(&mut *buffer)))
            })
            .collect();
        let current_tick = world.resource::<LocalTimeline>().tick();
        let boundary = start_tick - 1;
        world
            .resource_mut::<LocalTimeline>()
            .apply_delta(boundary - current_tick);
        let fixed_time = catch_up_fixed_time(&live_fixed, (current_tick - boundary).max(0) as u32);
        *world.resource_mut::<Time<Fixed>>() = fixed_time;
        *world.resource_mut::<Time>() = fixed_time.as_generic();
        world.trigger(P2PCatchUpReplay {
            session_start_tick: start_tick,
            target_tick,
        });
        world.flush();
        for (hash, receiver, buffer) in live {
            let mut entity = world.spawn((CatchUpLiveBuffer { hash }, buffer));
            if let Some(receiver) = receiver {
                entity.insert(RemoteInputTarget { hash, receiver });
            }
        }
        let mut replay = world.resource_mut::<CatchUpReplay>();
        replay.replayed_to = Some(boundary);
        replay.fixed_time = Some(fixed_time);
    } else {
        *world.resource_mut::<Time<Fixed>>() =
            world.resource::<CatchUpReplay>().fixed_time.unwrap();
    }

    let mut live_buffers =
        world.query::<(&CatchUpLiveBuffer, &InputBuffer<S::Snapshot, S::Action>)>();
    let history = world.resource::<InputHistory<S>>();
    let frontier = live_buffers
        .iter(world)
        .map(|(target, buffer)| {
            history
                .recorded_until
                .get(&target.hash)
                .copied()
                .max(buffer.last_remote_tick)
        })
        .collect::<Option<SmallVec<[Tick; 4]>>>()
        .and_then(replay_target);
    if frontier.is_none() {
        world.resource_mut::<CatchUpReplay>().failed = true;
    }
    let batch_start = world.resource::<LocalTimeline>().tick();
    let batch_end = frontier.map_or(batch_start, |frontier| {
        frontier.min(batch_start + budget.min(i32::MAX as u32) as i32)
    });
    let joins: SmallVec<[SessionJoin; 4]> = world
        .resource::<InputHistory<S>>()
        .joins
        .iter()
        .filter(|join| join.tick > batch_start && join.tick <= batch_end)
        .copied()
        .collect();
    for _ in 0..(batch_end - batch_start).max(0) {
        let next_tick = world.resource::<LocalTimeline>().tick() + 1;
        for join in joins.iter().filter(|join| join.tick == next_tick) {
            let link = world
                .query::<(Entity, &RemoteId)>()
                .iter(world)
                .find_map(|(entity, remote)| (remote.0 == join.peer).then_some(entity));
            world.trigger(lightyear_p2p::P2PJoined {
                peer_id: join.peer,
                activate_tick: join.tick,
                local_is_joiner: false,
                link,
            });
            world.flush();
        }
        let covered = players.iter(world).next().is_some()
            && world
                .resource::<CatchUpReplayInputs<S>>()
                .by_tick
                .get(&next_tick)
                .is_some_and(|row| {
                    players.iter(world).all(|(target, _)| {
                        target
                            .hash
                            .is_some_and(|hash| row.iter().any(|(key, _)| *key == hash))
                    })
                });
        if !covered {
            tracing::error!(
                ?next_tick,
                "catch-up input history does not cover the replay tick"
            );
            world.resource_mut::<CatchUpReplay>().failed = true;
            break;
        }
        *world.resource_mut::<Time>() = world.resource::<Time<Fixed>>().as_generic();
        world.run_schedule(FixedMain);
        if verify && world.resource::<LocalTimeline>().tick() == target_tick {
            let mut state = bevy_ecs::system::SystemState::<
                crate::archetypes::ChecksumWorld<'_, '_, true>,
            >::new(world);
            let mut checksum_world = state
                .get_mut(world)
                .expect("checksum registries are installed");
            checksum_world.update_archetypes();
            let (checksum, coverage) = crate::checksum::compute_history_checksum_with_coverage(
                &mut checksum_world,
                target_tick,
            );
            let checksum = (coverage.entries > 0).then_some(checksum);
            let mut replay = world.resource_mut::<CatchUpReplay>();
            replay.local_checksum = checksum;
            if checksum != replay.donor_checksum {
                replay.failed = true;
                break;
            }
        }
        let timestep = world.resource::<Time<Fixed>>().timestep();
        world.resource_mut::<Time<Fixed>>().advance_by(timestep);
    }
    let cursor = world.resource::<LocalTimeline>().tick();
    let fixed_time = *world.resource::<Time<Fixed>>();
    *world.resource_mut::<Time<Fixed>>() = live_fixed;
    *world.resource_mut::<Time>() = live_time;
    let mut replay = world.resource_mut::<CatchUpReplay>();
    replay.replayed_to = Some(cursor);
    replay.fixed_time = Some(fixed_time);
    replay.complete = frontier.is_some_and(|frontier| cursor >= frontier) && cursor >= target_tick;
    tracing::debug!(
        ?cursor,
        ?frontier,
        replayed_ticks = cursor - batch_start,
        "replayed catch-up batch"
    );
}

fn preserve_future_inputs<S: ActionStateSequence>(world: &mut World, current_tick: Tick) {
    let Some(mut inputs) = world.get_resource_mut::<CatchUpReplayInputs<S>>() else {
        return;
    };
    let inputs = core::mem::take(&mut *inputs);
    let mut players = world.query::<(&PreSpawned, &mut InputBuffer<S::Snapshot, S::Action>)>();
    for (tick, rows) in inputs.by_tick.range((
        core::ops::Bound::Excluded(current_tick),
        core::ops::Bound::Unbounded,
    )) {
        for (target, mut buffer) in players.iter_mut(world) {
            if let Some((_, snapshot)) = rows.iter().find(|(hash, _)| Some(*hash) == target.hash) {
                buffer.set_raw(*tick, Some(snapshot.clone()));
                buffer.last_remote_tick =
                    Some(buffer.last_remote_tick.map_or(*tick, |old| old.max(*tick)));
            }
        }
    }
}

fn release_live_targets<S: ActionStateSequence>(world: &mut World, restore_buffers: bool) {
    let targets: Vec<_> = world
        .query_filtered::<(Entity, &CatchUpLiveBuffer), With<InputBuffer<S::Snapshot, S::Action>>>()
        .iter(world)
        .map(|(entity, target)| (entity, target.hash))
        .collect();
    for (entity, hash) in targets {
        if restore_buffers {
            let buffer = world
                .entity_mut(entity)
                .take::<InputBuffer<S::Snapshot, S::Action>>()
                .unwrap();
            let target = world
                .query::<(Entity, &PreSpawned)>()
                .iter(world)
                .find_map(|(entity, target)| (target.hash == Some(hash)).then_some(entity));
            if let Some(target) = target {
                world.entity_mut(target).insert(buffer);
            }
        }
        world.despawn(entity);
    }
}

/// A `Time<Fixed>` rewound far enough to run `num_ticks` fixed steps ending at the current tick.
///
/// The loop drives `FixedMain` itself and advances this clock by hand, so the clock only has to
/// start far enough back that the steps it is advanced through cover the replay.
fn catch_up_fixed_time(current: &Time<Fixed>, num_ticks: u32) -> Time<Fixed> {
    let mut fixed = Time::<Fixed>::from_duration(current.timestep());
    let rewound = current
        .elapsed()
        .saturating_sub(num_ticks.saturating_sub(1) * fixed.timestep());
    fixed.advance_to(rewound.saturating_sub(fixed.timestep()));
    fixed.advance_by(fixed.timestep());
    fixed
}

/// Check the replayed world against the donor's and report the catch-up to the session.
///
/// Incomplete batches remain pending. Completion requires reaching the current live frontier
/// after verifying the donor's checksum at the initial transfer boundary.
fn finish_catch_up_replay<S: ActionStateSequence>(world: &mut World) {
    let Some(replay) = world.get_resource::<CatchUpReplay>() else {
        return;
    };
    if !replay.failed && !replay.complete {
        return;
    }
    let replay = world.remove_resource::<CatchUpReplay>().unwrap();
    if let Some(mut transfer) = world.get_resource_mut::<CatchUpTransfer<S>>() {
        transfer.donor = None;
    }
    let verified = !replay.failed
        && replay.replayed_to.is_some()
        && replay
            .donor_checksum
            .is_none_or(|checksum| replay.local_checksum == Some(checksum));
    if !verified {
        release_live_targets::<S>(world, true);
        world
            .resource_mut::<CatchUpReplayInputs<S>>()
            .by_tick
            .clear();
        tracing::error!(target_tick = ?replay.target_tick, local_checksum = ?replay.local_checksum,
            donor_checksum = ?replay.donor_checksum, "catch-up did not reproduce the donor's state");
        world.trigger(P2PJoinCancel {
            reason: JoinRejectReason::CatchUpFailed,
        });
        return;
    }
    let caught_up_tick = world.resource::<LocalTimeline>().tick();
    preserve_future_inputs::<S>(world, caught_up_tick);
    release_live_targets::<S>(world, false);
    let history_start_tick = replay.start_tick - 1;
    tracing::info!(?caught_up_tick, ?history_start_tick, target_tick = ?replay.target_tick,
        checksum = ?replay.local_checksum, elapsed = ?replay.requested_at.elapsed(),
        "catch-up reached the live input frontier");
    world.trigger(P2PJoinCatchUpComplete {
        caught_up_tick,
        history_start_tick,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::query::QueryData;
    use bevy_ecs::system::{RunSystemOnce, SystemState};
    use core::time::Duration;
    use lightyear_inputs::input_message::Compressed;
    use lightyear_inputs::input_message::{ActionStateQueryData, InputSnapshot};
    use lightyear_replication::prelude::AppComponentExt;

    /// Marker identifying the action state a player is actively updating; part of the input type's
    /// contract even though the catch-up does not read it.
    #[derive(Component)]
    struct TestMarker;

    /// A minimal input type, so the flow can be tested without pulling in an input plugin.
    ///
    /// The catch-up is generic over the application's input type; what it needs from one is the
    /// ability to write a recorded tick back into a buffer, which is exactly
    /// [`ActionStateSequence::update_buffer`]. This exercises that contract.
    #[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    struct TestState(u32);

    impl InputSnapshot for TestState {
        fn decay_tick(&mut self, _tick_duration: Duration) {}
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    struct TestAction;

    impl ActionStateQueryData for TestState {
        type Mut = &'static mut Self;
        type MutItemInner<'w> = &'w mut TestState;
        type Main = TestState;
        type Bundle = TestState;

        fn as_read_only<'a, 'w: 'a, 's>(
            state: &'a <Self::Mut as QueryData>::Item<'w, 's>,
        ) -> <<Self::Mut as QueryData>::ReadOnly as QueryData>::Item<'a, 's> {
            state
        }

        fn into_inner<'w, 's>(
            mut_item: <Self::Mut as QueryData>::Item<'w, 's>,
        ) -> Self::MutItemInner<'w> {
            mut_item.into_inner()
        }

        fn as_mut(bundle: &mut Self::Bundle) -> Self::MutItemInner<'_> {
            bundle
        }

        fn base_value() -> Self::Bundle {
            TestState::default()
        }
    }

    /// The wire form: one state per tick, which is how the donor sends a recorded tick.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestSequence {
        states: Vec<Compressed<TestState>>,
    }

    impl TestSequence {
        fn one(state: TestState) -> Self {
            Self {
                states: alloc::vec![Compressed::Input(state)],
            }
        }
    }

    impl ActionStateSequence for TestSequence {
        type Action = TestAction;
        type Snapshot = TestState;
        type State = TestState;
        type Marker = TestMarker;

        fn len(&self) -> usize {
            self.states.len()
        }

        fn get_snapshots_from_message(
            self,
            _tick_duration: Duration,
        ) -> impl Iterator<Item = Compressed<Self::Snapshot>> {
            self.states.into_iter()
        }

        /// Mirrors the real input types: the window is trimmed to the first tick that has data,
        /// and the wire compression is **re-derived by comparing neighbours**.
        ///
        /// That re-derivation is what makes a record self-contained. `SameAsPrecedent` is a
        /// reference to the previous tick in *this* buffer, and the peer replaying the history has
        /// no such precedent, so a record carrying that reference would resolve to nothing on
        /// arrival. The first entry of a window is never `SameAsPrecedent`, so a one-tick record
        /// always carries its value.
        fn build_from_input_buffer(
            input_buffer: &InputBuffer<Self::Snapshot, Self::Action>,
            num_ticks: usize,
            end_tick: Tick,
        ) -> Option<Self> {
            let mut start_tick = Tick(
                end_tick
                    .0
                    .saturating_sub(num_ticks.saturating_sub(1) as u32),
            );
            while start_tick <= end_tick {
                if input_buffer.get(start_tick).is_some() {
                    break;
                }
                start_tick += 1;
            }
            if start_tick > end_tick {
                return None;
            }
            // The first entry is always resolved, exactly as the real types guarantee: it has no
            // predecessor inside the window to refer to.
            let mut states = Vec::with_capacity((end_tick - start_tick + 1) as usize);
            states.push(
                input_buffer
                    .get(start_tick)
                    .map_or(Compressed::Absent, |state| Compressed::Input(*state)),
            );
            let mut tick = start_tick + 1;
            while tick <= end_tick {
                states.push(match input_buffer.get(tick) {
                    None => Compressed::Absent,
                    Some(value) => match input_buffer.get(tick - 1u32) {
                        Some(previous) if previous == value => Compressed::SameAsPrecedent,
                        _ => Compressed::Input(*value),
                    },
                });
                tick += 1;
            }
            Some(Self { states })
        }

        fn to_snapshot(state: &TestState) -> Self::Snapshot {
            *state
        }

        fn from_snapshot(state: &mut TestState, snapshot: &Self::Snapshot) {
            *state = *snapshot;
        }
    }

    /// A joiner with one player entity and its input buffer.
    fn joiner_with_player(hash: u64) -> (App, Entity) {
        let mut app = App::new();
        let entity = app
            .world_mut()
            .spawn((
                PreSpawned::new(hash),
                InputBuffer::<TestState, TestAction>::default(),
            ))
            .id();
        app.insert_resource(StateRollbackMetadata::default());
        app.insert_resource(PredictionManager::default());
        app.init_resource::<lightyear_sync::prelude::InputTimelineConfig>();
        app.init_resource::<InputHistory<TestSequence>>();
        app.init_resource::<lightyear_core::prelude::LocalTimeline>();
        // The checksum param resolves the world through these registries.
        app.init_resource::<lightyear_replication::prelude::ComponentRegistry>();
        app.init_resource::<lightyear_prediction::prelude::PredictionRegistry>();
        (app, entity)
    }

    /// A deterministic peer with its normal prediction pipeline installed.
    ///
    /// Exercise catch-up with the same topology and synchronization gates as live play.
    fn peer_app(tick_duration: Duration) -> App {
        let mut app = App::new();
        app.add_plugins(lightyear_core::plugin::CorePlugins { tick_duration });
        app.add_plugins(lightyear_prediction::plugin::PredictionPlugin);
        app.insert_resource(PredictionManager {
            rollback_policy: lightyear_prediction::manager::RollbackPolicy {
                state: lightyear_prediction::manager::RollbackMode::Disabled,
                input: lightyear_prediction::manager::RollbackMode::Check,
                // A long replay must work without widening the ordinary rollback window.
                max_rollback_ticks: 4,
            },
            ..Default::default()
        });
        let link = app.world_mut().spawn_empty().id();
        let mut metadata = lightyear_connection::network_topology::NetworkingMetadata::default();
        metadata.mode = lightyear_connection::network_topology::NetworkTopology::P2P(
            lightyear_connection::p2p::P2PRoster::from_started_links([link]),
        );
        app.insert_resource(metadata);
        app.init_resource::<lightyear_sync::prelude::LocalTimelineSync>();
        app.init_resource::<lightyear_sync::prelude::InputTimelineConfig>();
        app.init_resource::<lightyear_prediction::prelude::LastConfirmedInput>();
        app.init_resource::<bevy_replicon::shared::replication::storage::ReplicationStorage>();
        app.init_resource::<InputHistory<TestSequence>>();
        app.init_resource::<JoinCatchUpConfig>();
        app.init_resource::<CatchUpReplayInputs<TestSequence>>();
        app.add_systems(FixedPreUpdate, stream_replay_inputs::<TestSequence>);
        app
    }

    /// Finish an app built by [`peer_app`], leaving it as a running application would be.
    fn finish_peer_app(app: &mut App) {
        app.finish();
        app.cleanup();
        app.world_mut()
            .resource_mut::<lightyear_sync::prelude::LocalTimelineSync>()
            .set_synced(true);
        // A running application has already ticked over, which consumes the initial topology-change
        // reset.
        app.world_mut().run_schedule(bevy_app::PreUpdate);
    }

    /// Spawn a player with an input buffer and a stable identity.
    fn spawn_player(app: &mut App, hash: u64) -> Entity {
        app.world_mut()
            .spawn((
                PreSpawned::new(hash),
                InputBuffer::<TestState, TestAction>::default(),
            ))
            .id()
    }

    /// Records for one player, one per tick, as the donor would have sent them.
    fn records_for(
        hash: u64,
        ticks: core::ops::RangeInclusive<u32>,
    ) -> Vec<RecordedInput<TestSequence>> {
        ticks
            .map(|tick| RecordedInput {
                hash,
                tick: Tick(tick),
                sequence: TestSequence::one(TestState(tick)),
            })
            .collect()
    }

    /// Every catch-up completion the app observed, in order.
    #[derive(Resource, Default)]
    struct Completions(Vec<P2PJoinCatchUpComplete>);

    /// Collect catch-up completions so a test can assert on what the session was told.
    fn collect_completions(app: &mut App) {
        app.init_resource::<Completions>();
        app.add_observer(
            |trigger: On<P2PJoinCatchUpComplete>, mut completions: ResMut<Completions>| {
                completions.0.push(*trigger.event());
            },
        );
    }

    /// Place the local timeline at `tick`.
    fn set_timeline_tick(app: &mut App, tick: u32) {
        let current = app
            .world()
            .resource::<lightyear_core::prelude::LocalTimeline>()
            .tick();
        app.world_mut()
            .resource_mut::<lightyear_core::prelude::LocalTimeline>()
            .apply_delta(tick as i32 - current.0 as i32);
    }

    /// Record into a history with the same bound the tests use elsewhere.
    fn record(history: &mut InputHistory<TestSequence>, hash: u64, tick: u32) -> bool {
        history.record(hash, Tick(tick), TestSequence::one(TestState(tick)), 100)
    }

    /// Drive [`apply_catch_up`] the way the transport does: with the real system parameters.
    fn run_catch_up(
        app: &mut App,
        records: &[RecordedInput<TestSequence>],
        start: Tick,
        target: Tick,
    ) {
        run_catch_up_at_session_start(app, records, start, target, start);
    }

    /// As [`run_catch_up`], with the donor's session start reported explicitly.
    fn run_catch_up_at_session_start(
        app: &mut App,
        records: &[RecordedInput<TestSequence>],
        start: Tick,
        target: Tick,
        session_start: Tick,
    ) {
        run_catch_up_at_session_start_with_checksum(
            app,
            records,
            start,
            target,
            session_start,
            None,
        );
    }

    /// As [`run_catch_up_at_session_start`], also reporting what the donor's world hashed to.
    fn run_catch_up_at_session_start_with_checksum(
        app: &mut App,
        records: &[RecordedInput<TestSequence>],
        start: Tick,
        target: Tick,
        session_start: Tick,
        donor_checksum: Option<u64>,
    ) {
        app.init_resource::<JoinCatchUpConfig>();
        app.init_resource::<CatchUpReplayInputs<TestSequence>>();
        let mut state = SystemState::<(
            Commands,
            ResMut<InputHistory<TestSequence>>,
            ResMut<CatchUpReplayInputs<TestSequence>>,
        )>::new(app.world_mut());
        {
            let (mut commands, mut history, mut inputs) = state.get_mut(app.world_mut()).unwrap();
            apply_catch_up::<TestSequence>(
                &mut commands,
                Tick(200),
                Duration::from_millis(16),
                records.to_vec(),
                Some(start),
                Some(target),
                Some(session_start),
                donor_checksum,
                &mut history,
                &mut inputs,
            );
        }
        state.apply(app.world_mut());
    }

    /// Every recorded tick is available to the replay, and the replay moves the buffer with it.
    ///
    /// The buffer is a fixed window, so the history cannot be written into it ahead of time — the
    /// early ticks would be evicted before the replay reached them, which is exactly what left the
    /// replay simulating nothing. The history is held beside the buffer and copied in one tick at a
    /// time as the replay arrives at that tick.
    #[test]
    fn the_replay_inputs_cover_every_recorded_tick_and_are_streamed_per_tick() {
        let (mut app, player) = joiner_with_player(7);
        let records = records_for(7, 100..=104);
        run_catch_up(&mut app, &records, Tick(100), Tick(104));

        // The history is held beside the buffers, not inside them.
        let replay_inputs = app.world().resource::<CatchUpReplayInputs<TestSequence>>();
        for tick in 100..=104u32 {
            assert!(
                replay_inputs.by_tick.contains_key(&Tick(tick)),
                "tick {tick} must be replayable"
            );
        }
        assert_eq!(replay_inputs.by_tick.len(), 5);
        // One tick past the transfer must not be invented.
        assert!(!replay_inputs.by_tick.contains_key(&Tick(105)));

        // Streaming a tick is what puts it in the buffer the input pipeline reads.
        app.add_systems(
            bevy_app::FixedPreUpdate,
            stream_replay_inputs::<TestSequence>,
        );
        for tick in 100..=104u32 {
            set_timeline_tick(&mut app, tick);
            app.world_mut().run_schedule(bevy_app::FixedPreUpdate);
            assert_eq!(
                app.world()
                    .entity(player)
                    .get::<InputBuffer<TestState, TestAction>>()
                    .unwrap()
                    .get(Tick(tick))
                    .copied(),
                Some(TestState(tick)),
                "the inputs for the tick being simulated must be in the buffer"
            );
        }
    }

    /// Inputs belong to the player that produced them.
    ///
    /// Streaming keys on `PreSpawned::hash`, so a mixed-up key would replay one player's inputs into
    /// another's body and the two peers would disagree from the first tick.
    #[test]
    fn each_players_inputs_go_to_their_own_buffer() {
        let mut app = App::new();
        let alice = app
            .world_mut()
            .spawn((
                PreSpawned::new(1),
                InputBuffer::<TestState, TestAction>::default(),
            ))
            .id();
        let bob = app
            .world_mut()
            .spawn((
                PreSpawned::new(2),
                InputBuffer::<TestState, TestAction>::default(),
            ))
            .id();
        app.insert_resource(StateRollbackMetadata::default());
        app.insert_resource(PredictionManager::default());
        app.init_resource::<lightyear_sync::prelude::InputTimelineConfig>();
        app.init_resource::<InputHistory<TestSequence>>();
        app.init_resource::<LocalTimeline>();

        let mut records = records_for(1, 10..=10);
        records.extend(records_for(2, 10..=10));
        // Distinguish the two players' inputs so a swap is visible.
        records[0].sequence = TestSequence::one(TestState(111));
        records[1].sequence = TestSequence::one(TestState(222));

        run_catch_up(&mut app, &records, Tick(10), Tick(10));

        app.add_systems(
            bevy_app::FixedPreUpdate,
            stream_replay_inputs::<TestSequence>,
        );
        set_timeline_tick(&mut app, 10);
        app.world_mut().run_schedule(bevy_app::FixedPreUpdate);

        assert_eq!(
            app.world()
                .entity(alice)
                .get::<InputBuffer<TestState, TestAction>>()
                .unwrap()
                .get(Tick(10))
                .copied(),
            Some(TestState(111))
        );
        assert_eq!(
            app.world()
                .entity(bob)
                .get::<InputBuffer<TestState, TestAction>>()
                .unwrap()
                .get(Tick(10))
                .copied(),
            Some(TestState(222))
        );
    }

    /// A transfer that matches no player must cancel rather than report a catch-up.
    ///
    /// Reporting success would let the join commit against a world that was never replayed.
    #[test]
    fn a_transfer_matching_no_player_cancels_the_join() {
        let mut app = peer_app(Duration::from_millis(10));
        finish_peer_app(&mut app);
        let player = spawn_player(&mut app, 7);
        let link = app.world_mut().spawn_empty().id();
        app.world_mut()
            .get_mut::<PreSpawned>(player)
            .unwrap()
            .receiver = Some(link);
        app.world_mut()
            .get_mut::<InputBuffer<TestState, TestAction>>(player)
            .unwrap()
            .set(Tick(200), TestState(42));
        collect_completions(&mut app);
        run_catch_up(&mut app, &records_for(999, 100..=104), Tick(100), Tick(104));
        app.add_systems(
            bevy_app::Update,
            (
                run_catch_up_replay::<TestSequence>,
                finish_catch_up_replay::<TestSequence>,
            )
                .chain(),
        );
        app.world_mut().run_schedule(bevy_app::Update);

        // Nothing was replayed, so no replay may be recorded and none may be reported.
        assert!(app.world().resource::<Completions>().0.is_empty());
        assert!(app.world().get_resource::<CatchUpReplay>().is_none());
        assert_eq!(
            app.world()
                .get::<InputBuffer<TestState, TestAction>>(player)
                .unwrap()
                .get(Tick(200)),
            Some(&TestState(42)),
            "cancellation must restore the live receive buffer"
        );
        assert_eq!(
            app.world_mut()
                .query::<&RemoteInputTarget>()
                .iter(app.world())
                .count(),
            0
        );
    }

    /// The replay is only reported once it has actually run.
    ///
    /// Reporting on a replay that never happened would tell the session a joiner had caught up when
    /// its world was never simulated forward, and the session would commit it against that world.
    #[test]
    fn the_catch_up_is_reported_only_after_the_replay_has_run() {
        let mut app = peer_app(Duration::from_millis(10));
        finish_peer_app(&mut app);
        // Model the newcomer's live timeline before replay rewinds it to the session start.
        app.world_mut()
            .resource_mut::<lightyear_core::prelude::LocalTimeline>()
            .apply_delta(110);
        let _player = spawn_player(&mut app, 7);
        let records = records_for(7, 100..=104);
        run_catch_up(&mut app, &records, Tick(100), Tick(104));

        collect_completions(&mut app);
        app.add_systems(
            bevy_app::Update,
            (
                run_catch_up_replay::<TestSequence>,
                finish_catch_up_replay::<TestSequence>,
            )
                .chain(),
        );

        // An incomplete history cannot be simulated or reported as caught up.
        app.world_mut()
            .resource_mut::<CatchUpReplayInputs<TestSequence>>()
            .by_tick
            .clear();
        app.world_mut().run_schedule(bevy_app::Update);

        assert!(
            app.world().resource::<Completions>().0.is_empty(),
            "a catch-up must not be reported while the replay it asked for has not run"
        );
        // The aborted catch-up is dropped rather than retried, so nothing is left behind.
        assert!(app.world().get_resource::<CatchUpReplay>().is_none());
    }

    /// The recorder's own path: `build_from_input_buffer` out, `update_buffer` back in.
    ///
    /// The flow tests build their records by hand, so they never exercise the pair of functions the
    /// transfer actually uses. This is the round trip that decides whether a joiner's buffer ends up
    /// holding the session's inputs or merely something well-typed.
    #[test]
    fn a_recorded_tick_round_trips_into_a_fresh_buffer() {
        let tick_duration = Duration::from_millis(16);
        let mut source = InputBuffer::<TestState, TestAction>::default();
        // Two ticks holding the same input, then a change. `InputBuffer::set` compresses the repeat
        // to `SameAsPrecedent`, so this is the case that has to survive the trip.
        source.set(Tick(100), TestState(1));
        source.set(Tick(101), TestState(1));
        source.set(Tick(102), TestState(2));

        let records: Vec<RecordedInput<TestSequence>> = (100..=102)
            .map(|t| {
                let tick = Tick(t);
                RecordedInput {
                    hash: 7,
                    tick,
                    sequence: TestSequence::build_from_input_buffer(&source, 1, tick)
                        .expect("the tick is inside the buffer"),
                }
            })
            .collect();

        let mut fresh = InputBuffer::<TestState, TestAction>::default();
        for record in &records {
            record
                .sequence
                .clone()
                .update_buffer(&mut fresh, record.tick, tick_duration);
        }

        for t in 100..=102 {
            assert_eq!(
                fresh.get(Tick(t)),
                source.get(Tick(t)),
                "tick {t} must survive the round trip"
            );
        }
    }

    /// A component the simulation integrates from input.
    ///
    /// Kept local — not replicated — which is what a deterministic peer's physics state is.
    #[derive(
        Component, Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize,
    )]
    struct ReplayPosition(i32);

    /// The simulation: this tick's input moves this player.
    fn integrate_input(
        timeline: Res<LocalTimeline>,
        mut players: Query<(&mut ReplayPosition, &InputBuffer<TestState, TestAction>)>,
    ) {
        let tick = timeline.tick();
        for (mut position, buffer) in &mut players {
            if let Some(state) = buffer.get(tick) {
                position.0 += state.0 as i32;
            }
        }
    }

    /// The point of the whole catch-up: a peer that starts from the initial world and replays the
    /// session's inputs must end up where a peer that simulated them live did.
    #[test_log::test]
    fn a_replay_reproduces_the_session_from_a_freshly_built_world() {
        let tick_duration = Duration::from_millis(10);
        let mut app = App::new();
        app.add_plugins(lightyear_core::plugin::CorePlugins { tick_duration });
        app.add_plugins(lightyear_prediction::plugin::PredictionPlugin);
        app.insert_resource(PredictionManager {
            rollback_policy: lightyear_prediction::manager::RollbackPolicy {
                state: lightyear_prediction::manager::RollbackMode::Disabled,
                input: lightyear_prediction::manager::RollbackMode::Check,
                // A long replay must work without widening the ordinary rollback window.
                max_rollback_ticks: 4,
            },
            ..Default::default()
        });
        // Enable ordinary prediction as well, so this exercises replay's isolation from it.
        let link = app.world_mut().spawn_empty().id();
        let mut metadata = lightyear_connection::network_topology::NetworkingMetadata::default();
        metadata.mode = lightyear_connection::network_topology::NetworkTopology::P2P(
            lightyear_connection::p2p::P2PRoster::from_started_links([link]),
        );
        app.insert_resource(metadata);
        app.init_resource::<lightyear_sync::prelude::LocalTimelineSync>();
        app.init_resource::<lightyear_sync::prelude::InputTimelineConfig>();
        app.init_resource::<lightyear_prediction::prelude::LastConfirmedInput>();
        app.init_resource::<bevy_replicon::shared::replication::storage::ReplicationStorage>();
        app.init_resource::<InputHistory<TestSequence>>();
        app.init_resource::<JoinCatchUpConfig>();
        app.init_resource::<CatchUpReplayInputs<TestSequence>>();
        lightyear_prediction::registry::PredictionBuilderExt::local_rollback(
            app.component::<ReplayPosition>(),
        );
        app.add_systems(bevy_app::FixedUpdate, integrate_input);
        app.add_systems(
            bevy_app::FixedPreUpdate,
            stream_replay_inputs::<TestSequence>,
        );
        app.add_systems(bevy_app::PreUpdate, run_catch_up_replay::<TestSequence>);
        app.finish();
        app.cleanup();
        app.world_mut()
            .resource_mut::<lightyear_sync::prelude::LocalTimelineSync>()
            .set_synced(true);
        // Consume the initial topology-change reset before constructing the replay fixture.
        app.world_mut().run_schedule(bevy_app::PreUpdate);

        let player = app
            .world_mut()
            .spawn((
                PreSpawned::new(7),
                ReplayPosition::default(),
                InputBuffer::<TestState, TestAction>::default(),
                lightyear_prediction::prelude::PredictionHistory::<ReplayPosition>::default(),
            ))
            .id();

        // Simulate a live session. The player's input is originated by the application, exactly as
        // a local player's would be; it repeats every few ticks so the buffer compresses to
        // `SameAsPrecedent` the way a real one does.
        const TICKS: u32 = 12;
        let mut truth = Vec::new();
        let mut records = Vec::new();
        for _ in 0..TICKS {
            let tick = app.world().resource::<LocalTimeline>().tick() + 1;
            app.world_mut()
                .entity_mut(player)
                .get_mut::<InputBuffer<TestState, TestAction>>()
                .unwrap()
                .set(tick, TestState((tick.0 / 3) % 4 + 1));
            app.world_mut().run_schedule(bevy_app::FixedMain);

            let buffer = app
                .world()
                .entity(player)
                .get::<InputBuffer<TestState, TestAction>>()
                .unwrap();
            let sequence = TestSequence::build_from_input_buffer(buffer, 1, tick)
                .expect("the tick was just originated");
            records.push(RecordedInput {
                hash: 7,
                tick,
                sequence,
            });
            truth.push((
                tick,
                app.world()
                    .entity(player)
                    .get::<ReplayPosition>()
                    .unwrap()
                    .0,
            ));
        }

        let start_tick = truth[0].0;
        let (last_tick, expected) = *truth.last().unwrap();
        assert_ne!(expected, 0, "the live session must actually have moved");

        // Now rebuild the world the way a joiner does: initial value, no prediction history, and no
        // inputs. Everything the simulation needs has to come from the catch-up.
        app.world_mut().entity_mut(player).insert((
            ReplayPosition::default(),
            lightyear_prediction::prelude::PredictionHistory::<ReplayPosition>::default(),
            InputBuffer::<TestState, TestAction>::default(),
        ));

        let mut state = SystemState::<(
            Commands,
            ResMut<InputHistory<TestSequence>>,
            ResMut<CatchUpReplayInputs<TestSequence>>,
        )>::new(app.world_mut());
        {
            let (mut commands, mut history, mut inputs) = state.get_mut(app.world_mut()).unwrap();
            apply_catch_up::<TestSequence>(
                &mut commands,
                last_tick,
                tick_duration,
                records,
                Some(start_tick),
                Some(last_tick),
                // The world was rebuilt at the session's first tick, so the history reaches it.
                Some(start_tick),
                None,
                &mut history,
                &mut inputs,
            );
        }
        state.apply(app.world_mut());
        // Past the catch-up the Links are up, so the timeline is synchronized.
        app.world_mut()
            .resource_mut::<lightyear_sync::prelude::LocalTimelineSync>()
            .set_synced(true);
        // A session larger than the frame budget requires multiple batches.
        for _ in 0..2 {
            run_catch_up_replay::<TestSequence>(app.world_mut());
        }

        let replayed = app
            .world()
            .entity(player)
            .get::<ReplayPosition>()
            .unwrap()
            .0;
        assert_eq!(
            replayed, expected,
            "replaying the session from {start_tick:?} must land on the live state at {last_tick:?}"
        );
    }

    #[test_log::test]
    fn replay_checks_the_target_before_pruning_and_preserves_future_live_input() {
        use core::hash::{Hash, Hasher};
        use lightyear_prediction::registry::PredictionBuilderExt;

        let mut app = peer_app(Duration::from_millis(10));
        app.add_plugins((
            bevy_state::app::StatesPlugin,
            bevy_replicon::shared::RepliconSharedPlugin {
                auth_method: bevy_replicon::prelude::AuthMethod::None,
            },
            lightyear_prediction::plugin::PredictionMarkerPlugin,
        ));
        app.component::<ReplayPosition>()
            .add_default_hash()
            .predict();
        app.add_observer(
            |_: On<P2PCatchUpReplay>,
             mut commands: Commands,
             players: Query<Entity, With<ReplayPosition>>| {
                for entity in &players {
                    commands.entity(entity).despawn();
                }
                commands.spawn((
                    PreSpawned::new(7),
                    InputBuffer::<TestState, TestAction>::default(),
                    ReplayPosition::default(),
                    crate::Deterministic,
                    lightyear_prediction::rollback::DeterministicPredicted {
                        skip_despawn: true,
                        enable_rollback_after: 0,
                    },
                ));
            },
        );
        app.add_systems(bevy_app::FixedUpdate, integrate_input);
        collect_completions(&mut app);
        finish_peer_app(&mut app);
        let player = spawn_player(&mut app, 7);
        app.world_mut().entity_mut(player).insert((
            crate::Deterministic,
            lightyear_prediction::rollback::DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 0,
            },
            ReplayPosition::default(),
        ));
        set_timeline_tick(&mut app, 50);
        let mut hasher = seahash::SeaHasher::new();
        ReplayPosition(55).hash(&mut hasher);
        run_catch_up_at_session_start_with_checksum(
            &mut app,
            &records_for(7, 1..=50),
            Tick(1),
            Tick(10),
            Tick(1),
            Some(hasher.finish()),
        );
        {
            let mut entity = app.world_mut().entity_mut(player);
            let mut buffer = entity
                .get_mut::<InputBuffer<TestState, TestAction>>()
                .unwrap();
            buffer.set(Tick(51), TestState(99));
            buffer.last_remote_tick = Some(Tick(51));
        }
        app.world_mut()
            .run_system_once(capture_live_inputs::<TestSequence>)
            .unwrap();
        for _ in 0..6 {
            run_catch_up_replay::<TestSequence>(app.world_mut());
        }
        let mut players = app
            .world_mut()
            .query_filtered::<Entity, With<ReplayPosition>>();
        let player = players.single(app.world()).unwrap();
        app.add_systems(bevy_app::Update, finish_catch_up_replay::<TestSequence>);
        app.world_mut().run_schedule(bevy_app::Update);
        assert_eq!(
            app.world().resource::<Completions>().0.as_slice(),
            &[P2PJoinCatchUpComplete {
                caught_up_tick: Tick(51),
                history_start_tick: Tick(0),
            }]
        );
        assert_eq!(
            app.world().get::<ReplayPosition>(player),
            Some(&ReplayPosition(1374))
        );
        {
            let mut entity = app.world_mut().entity_mut(player);
            let mut buffer = entity
                .get_mut::<InputBuffer<TestState, TestAction>>()
                .unwrap();
            buffer.set(Tick(49), TestState(0));
            buffer.set(Tick(50), TestState(0));
        }
        let manager = app.world().resource::<PredictionManager>();
        manager.earliest_mismatch_input.tick.set_if_lower(Tick(49));
        manager
            .earliest_mismatch_input
            .has_mismatches
            .store(true, bevy_platform::sync::atomic::Ordering::Relaxed);
        app.world_mut().run_schedule(bevy_app::PreUpdate);
        assert_eq!(
            app.world().get::<ReplayPosition>(player),
            Some(&ReplayPosition(1275)),
            "the first input rollback must restore the rebuilt player before replaying its tail"
        );
    }

    #[test]
    fn incremental_replay_retains_live_overlap_and_chases_the_slowest_peer() {
        let mut app = peer_app(Duration::from_millis(10));
        app.add_systems(bevy_app::FixedUpdate, integrate_input);
        collect_completions(&mut app);
        #[derive(Resource, Default)]
        struct Rebuilds(u32);
        app.init_resource::<Rebuilds>();
        app.add_observer(
            |_: On<P2PCatchUpReplay>,
             mut rebuilds: ResMut<Rebuilds>,
             mut players: Query<&mut ReplayPosition>| {
                rebuilds.0 += 1;
                for mut position in &mut players {
                    *position = ReplayPosition::default();
                }
            },
        );
        finish_peer_app(&mut app);
        let alice = spawn_player(&mut app, 7);
        let bob = spawn_player(&mut app, 8);
        for entity in [alice, bob] {
            let link = app.world_mut().spawn_empty().id();
            app.world_mut()
                .entity_mut(entity)
                .insert(ReplayPosition::default());
            app.world_mut()
                .get_mut::<PreSpawned>(entity)
                .unwrap()
                .receiver = Some(link);
        }
        app.insert_resource(CatchUpTransfer::<TestSequence> {
            donor: Some(alice),
            ..Default::default()
        });
        // A slow reliable transfer overlaps more live input than fits in either 64-slot ring.
        for tick in 101..=280 {
            for (entity, frontier) in [(alice, 280), (bob, 270)] {
                if tick > frontier {
                    continue;
                }
                let mut buffer = app
                    .world_mut()
                    .get_mut::<InputBuffer<TestState, TestAction>>(entity)
                    .unwrap();
                buffer.set(Tick(tick), TestState(tick));
                buffer.last_remote_tick = Some(Tick(tick));
            }
            app.world_mut()
                .run_system_once(capture_live_inputs::<TestSequence>)
                .unwrap();
            set_timeline_tick(&mut app, tick - 1);
            app.world_mut().run_schedule(FixedMain);
        }
        let mut records = records_for(7, 1..=120);
        records.extend(records_for(8, 1..=120));
        run_catch_up(&mut app, &records, Tick(1), Tick(120));
        set_timeline_tick(&mut app, 280);
        run_catch_up_replay::<TestSequence>(app.world_mut());
        finish_catch_up_replay::<TestSequence>(app.world_mut());
        assert_eq!(app.world().resource::<LocalTimeline>().tick(), Tick(10));
        assert!(app.world().resource::<Completions>().0.is_empty());

        let mut fast = 280;
        let mut slow = 270;
        for _ in 0..40 {
            let before = app.world().resource::<LocalTimeline>().tick();
            let mut buffers = app
                .world_mut()
                .query::<(&CatchUpLiveBuffer, &mut InputBuffer<TestState, TestAction>)>();
            for (target, mut buffer) in buffers.iter_mut(app.world_mut()) {
                let (from, to) = if target.hash == 7 {
                    (fast + 1, fast + 2)
                } else {
                    (slow + 1, slow + 1)
                };
                for tick in from..=to {
                    buffer.set(Tick(tick), TestState(tick));
                }
                buffer.last_remote_tick = Some(Tick(to));
            }
            fast += 2;
            slow += 1;
            app.world_mut()
                .run_system_once(capture_live_inputs::<TestSequence>)
                .unwrap();
            run_catch_up_replay::<TestSequence>(app.world_mut());
            finish_catch_up_replay::<TestSequence>(app.world_mut());
            let cursor = app.world().resource::<LocalTimeline>().tick();
            assert!(
                (1..=10).contains(&(cursor - before)),
                "one frame must obey its replay budget"
            );
            assert!(
                cursor <= Tick(slow),
                "the fast peer cannot authorize prediction past the slow one"
            );
            let expected = ReplayPosition((cursor.0 * (cursor.0 + 1) / 2) as i32);
            for entity in [alice, bob] {
                assert_eq!(app.world().get::<ReplayPosition>(entity), Some(&expected));
            }
            if !app.world().resource::<Completions>().0.is_empty() {
                break;
            }
        }
        assert_eq!(app.world().resource::<Rebuilds>().0, 1);
        assert_eq!(
            app.world().resource::<Completions>().0.as_slice(),
            &[P2PJoinCatchUpComplete {
                caught_up_tick: Tick(slow),
                history_start_tick: Tick(0)
            }]
        );
        assert_eq!(
            app.world_mut()
                .query::<&RemoteInputTarget>()
                .iter(app.world())
                .count(),
            0
        );
        assert_eq!(
            app.world()
                .get::<InputBuffer<TestState, TestAction>>(alice)
                .unwrap()
                .get(Tick(fast)),
            Some(&TestState(fast)),
            "the faster peer's suffix must survive activation"
        );
        let history = app.world().resource::<InputHistory<TestSequence>>();
        let archived: u32 = history
            .records
            .iter()
            .filter(|record| record.tick <= Tick(slow))
            .flat_map(|record| {
                record
                    .sequence
                    .clone()
                    .get_snapshots_from_message(Duration::from_millis(10))
            })
            .map(|snapshot| match snapshot {
                Compressed::Input(state) => state.0,
                _ => panic!("unresolved history"),
            })
            .sum();
        assert_eq!(
            archived,
            slow * (slow + 1),
            "a subsequent joiner must replay the same world"
        );
    }

    /// A replay that never ran must not be reported as a catch-up.
    #[test]
    fn a_replay_that_never_ran_is_not_reported_as_a_catch_up() {
        let (mut app, _player) = joiner_with_player(7);
        app.insert_resource(LocalTimeline::default());
        let records = records_for(7, 100..=104);
        run_catch_up(&mut app, &records, Tick(100), Tick(104));
        collect_completions(&mut app);
        app.add_systems(bevy_app::Update, finish_catch_up_replay::<TestSequence>);

        // The replay system is never given the chance to run.
        app.update();

        assert!(
            app.world().resource::<Completions>().0.is_empty(),
            "a replay that never ran must not be reported as a catch-up"
        );
    }

    /// An application that installed its own configuration keeps it.
    ///
    /// Catch-up can be configured on its own, without going through the plugin, and the plugin must
    /// not overwrite that.
    #[test]
    fn an_application_configuration_wins_over_the_plugins() {
        let mut app = App::new();
        app.insert_resource(JoinCatchUpConfig {
            enabled: true,
            max_history_ticks: 999,
            ..Default::default()
        });
        app.add_plugins(JoinCatchUpPlugin::<TestSequence>::new(JoinCatchUpConfig {
            enabled: false,
            max_history_ticks: 12_345,
            ..Default::default()
        }));

        assert_eq!(
            app.world()
                .resource::<JoinCatchUpConfig>()
                .max_history_ticks,
            999
        );
    }

    /// A history that begins after the session did must be refused, not replayed.
    ///
    /// The peer builds the world as it was at the session's first gameplay tick and replays from
    /// there. If the history starts later, the ticks in between are simulated from inputs nobody
    /// recorded, and the two peers diverge while everything still reports success. That is what a
    /// live run did: the session started at tick 150, the donor's history at 153, and the joiner
    /// announced a replay that could not have been right.
    #[test]
    fn a_history_that_begins_after_the_session_is_refused() {
        let (mut app, _player) = joiner_with_player(7);
        app.insert_resource(LocalTimeline::default());
        let records = records_for(7, 153..=160);
        collect_completions(&mut app);
        app.add_systems(bevy_app::Update, finish_catch_up_replay::<TestSequence>);

        run_catch_up_at_session_start(
            &mut app,
            &records,
            Tick(153),
            Tick(160),
            // The donor reports a session that began three ticks before its history does.
            Tick(150),
        );

        assert!(
            app.world().get_resource::<CatchUpReplay>().is_none(),
            "and it must not be reported as a catch-up either"
        );
    }

    /// A history that reaches the session's start is accepted.
    #[test]
    fn a_history_that_reaches_the_session_start_is_accepted() {
        let mut app = peer_app(Duration::from_millis(10));
        finish_peer_app(&mut app);
        app.world_mut()
            .resource_mut::<lightyear_core::prelude::LocalTimeline>()
            .apply_delta(200);
        let _player = spawn_player(&mut app, 7);
        let records = records_for(7, 100..=110);
        collect_completions(&mut app);
        app.add_systems(bevy_app::Update, finish_catch_up_replay::<TestSequence>);

        // The history begins exactly at the session's first gameplay tick, which is the boundary
        // the world was built at.
        run_catch_up_at_session_start(&mut app, &records, Tick(100), Tick(110), Tick(100));

        assert_eq!(
            app.world().resource::<CatchUpReplay>().start_tick,
            Tick(100),
            "the replay starts at the session's first gameplay tick"
        );
    }

    /// A replay whose world disagrees with the donor's is refused, not reported as a catch-up.
    ///
    /// Agreeing on the inputs and the tick is not the same as agreeing on the world: a replay that
    /// is subtly wrong still produces the right shape of state, and every later stage would be built
    /// on it. Nothing else in the exchange would notice.
    #[test]
    fn a_replay_that_disagrees_with_the_donor_is_refused() {
        let mut app = peer_app(Duration::from_millis(10));
        finish_peer_app(&mut app);
        app.world_mut()
            .resource_mut::<lightyear_core::prelude::LocalTimeline>()
            .apply_delta(120);
        let _player = spawn_player(&mut app, 7);
        let records = records_for(7, 100..=110);
        collect_completions(&mut app);
        app.add_systems(bevy_app::Update, finish_catch_up_replay::<TestSequence>);

        run_catch_up_at_session_start_with_checksum(
            &mut app,
            &records,
            Tick(100),
            Tick(110),
            Tick(100),
            // A donor whose world hashed to something this replay cannot produce.
            Some(0xDEAD_BEEF_DEAD_BEEF),
        );
        for _ in 0..2 {
            run_catch_up_replay::<TestSequence>(app.world_mut());
        }
        app.world_mut().run_schedule(bevy_app::PreUpdate);
        app.world_mut().run_schedule(bevy_app::Update);

        assert!(
            app.world().resource::<Completions>().0.is_empty(),
            "a replay that does not match the donor must not be reported as a catch-up"
        );
        assert!(
            app.world().get_resource::<CatchUpReplay>().is_none(),
            "and the failed catch-up must be abandoned rather than left pending"
        );
    }

    /// Recording keeps one entry per player and tick.
    #[test]
    fn recording_keeps_one_entry_per_player_and_tick() {
        let mut history = InputHistory::<TestSequence>::default();
        assert!(history.start_tick().is_none());

        assert!(record(&mut history, 1, 10));
        assert!(record(&mut history, 2, 10));
        assert!(record(&mut history, 1, 11));

        assert_eq!(history.len(), 3);
        assert_eq!(history.start_tick(), Some(Tick(10)));
        assert!(!history.is_truncated());
    }

    #[test]
    fn a_retransmitted_entry_is_not_recorded_twice() {
        let mut history = InputHistory::<TestSequence>::default();
        assert!(record(&mut history, 1, 10));
        assert!(!record(&mut history, 1, 10), "that tick is already held");
        assert_eq!(history.len(), 1);
        // Another player's inputs for the same tick are their own entry.
        assert!(record(&mut history, 2, 10));
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn an_out_of_order_arrival_is_still_recorded() {
        let mut history = InputHistory::<TestSequence>::default();
        // One player is ahead of another, so the lagging player's later arrival must still count.
        assert!(record(&mut history, 2, 20));
        assert!(record(&mut history, 1, 19));
        // The history starts at the earliest tick it covers, not the first one recorded.
        assert_eq!(history.start_tick(), Some(Tick(19)));
        assert_eq!(history.len(), 2);
        assert!(!history.is_truncated());
        // An entry below what that player already reported is stale and dropped.
        assert!(!record(&mut history, 2, 18));
    }

    #[test]
    fn the_history_refuses_to_grow_past_its_bound() {
        let mut history = InputHistory::<TestSequence>::default();
        assert!(record(&mut history, 1, 0));
        assert!(history.record(1, Tick(100), TestSequence::one(TestState(100)), 100));
        assert!(!history.is_truncated());

        // One tick further and the session can no longer be replayed from its start, so the entry
        // is refused and the history is marked unusable rather than growing without limit.
        assert!(!history.record(1, Tick(101), TestSequence::one(TestState(101)), 100));
        assert!(history.is_truncated());
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn the_replay_target_is_the_latest_tick_with_every_players_inputs() {
        // The slowest player bounds the replay: simulating past it would use inputs that have not
        // arrived, which is the divergence the target exists to prevent.
        assert_eq!(
            replay_target([Tick(50), Tick(40), Tick(55)]),
            Some(Tick(40))
        );
        assert_eq!(replay_target([Tick(50)]), Some(Tick(50)));
        assert_eq!(replay_target([]), None);
    }

    #[test]
    fn transfer_completion_waits_for_chunks_delivered_after_the_end() {
        let mut transfer = CatchUpTransfer::<TestSequence>::default();
        let records = records_for(7, 10..=14);
        transfer.records.extend_from_slice(&records[3..]);
        transfer.end = Some(CatchUpEnd {
            id: 1,
            joins: Vec::new(),
            start_tick: Some(Tick(10)),
            target_tick: Some(Tick(14)),
            session_start_tick: Some(Tick(10)),
            checksum: Some(1),
            record_count: records.len(),
            refusal: None,
        });
        assert!(transfer.take_complete().is_none());
        transfer.records.extend_from_slice(&records[..3]);
        let (complete, end) = transfer.take_complete().unwrap();
        let (mut app, player) = joiner_with_player(7);
        run_catch_up(
            &mut app,
            &complete,
            end.start_tick.unwrap(),
            end.target_tick.unwrap(),
        );
        app.add_systems(FixedPreUpdate, stream_replay_inputs::<TestSequence>);
        for tick in 10..=14 {
            set_timeline_tick(&mut app, tick);
            app.world_mut().run_schedule(FixedPreUpdate);
            assert_eq!(
                app.world()
                    .entity(player)
                    .get::<InputBuffer<TestState, TestAction>>()
                    .unwrap()
                    .get(Tick(tick)),
                Some(&TestState(tick))
            );
        }
        assert!(
            transfer.take_complete().is_none(),
            "completion is consumed once"
        );
    }
}
