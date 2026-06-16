use alloc::vec::Vec;
use bevy_app::{App, PostUpdate, PreUpdate};
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
use bevy_replicon::prelude::RepliconTick;
use core::time::Duration;
use lightyear_connection::client::Client;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_inputs::client::InputSystems;
use lightyear_inputs::input_buffer::InputBuffer;
use lightyear_inputs::input_message::ActionStateSequence;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{MessageSender, RemoteEvent};
use lightyear_prediction::prelude::{PredictionSystems, StateRollbackMetadata};
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::prelude::{ConfirmHistory, ReplicationSystems};
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use tracing::debug;

use crate::mode::CatchUpMode;

use super::{
    AwaitingCatchUpSnapshot, CatchUpGated, CatchUpRequest, CatchUpSnapshotReady, CatchUpSystems,
};

/// Client-side timeout for an in-flight catch-up request.
///
/// This is intentionally a hard panic by default. A client that requested a
/// state-based catch-up but never activates it is running from stale state and
/// should fail loudly instead of silently producing misleading checksums.
#[derive(Resource, Clone, Copy, Debug)]
pub struct CatchUpClientTimeout {
    pub duration: Duration,
}

impl Default for CatchUpClientTimeout {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(1),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PendingCatchUpSnapshot {
    pub(crate) server_tick: Tick,
    pub(crate) replicon_tick: RepliconTick,
}

/// Client-side catch-up state stored on the client link entity.
///
/// The manager owns all per-client metadata for the initial catch-up flow:
/// input coverage, request retry state, accepted snapshot state, and completion
/// status.
#[derive(Component, Debug)]
pub struct CatchUpManager {
    pub(crate) completed: bool,
    pub(crate) pending_snapshot: Option<PendingCatchUpSnapshot>,
    pub(crate) input_checks_this_frame: usize,
    pub(crate) input_safe_tick: Option<Tick>,
    pub(crate) request_sent_at_tick: Option<Tick>,
    pub(crate) request_input_safe_tick: Option<Tick>,
    pub(crate) last_emitted_replicon_tick: Option<RepliconTick>,
    pub(crate) suppress_checksums: bool,
}

impl Default for CatchUpManager {
    fn default() -> Self {
        Self {
            completed: false,
            pending_snapshot: None,
            input_checks_this_frame: 0,
            input_safe_tick: Some(Tick(u32::MAX)),
            request_sent_at_tick: None,
            request_input_safe_tick: None,
            last_emitted_replicon_tick: None,
            suppress_checksums: false,
        }
    }
}

impl CatchUpManager {
    /// Returns true while the client is running with intentionally stale
    /// deterministic state and checksum sends should be suppressed.
    pub fn suppresses_checksums(&self) -> bool {
        self.suppress_checksums
    }
}

pub(crate) fn register_catchup<S: ActionStateSequence>(app: &mut App) {
    app.add_systems(
        PreUpdate,
        update_client_catchup_input_readiness::<S>
            .in_set(CatchUpSystems::CheckClientReplayReadiness)
            .run_if(initial_catchup_is_active),
    );
}

pub(crate) fn build(app: &mut App) {
    app.init_resource::<CatchUpClientTimeout>();
    app.register_required_components::<Client, CatchUpManager>();
    app.add_observer(receive_catch_up_snapshot_ready);
    app.configure_sets(
        PreUpdate,
        CatchUpSystems::ResetReadiness
            .after(MessageSystems::Receive)
            .after(InputSystems::ReceiveInputMessages),
    );
    app.configure_sets(
        PreUpdate,
        (
            CatchUpSystems::DetectSnapshotReady,
            CatchUpSystems::CheckClientReplayReadiness,
            CatchUpSystems::FinalizeSnapshot,
        )
            .chain()
            .after(ReplicationSystems::Receive)
            .after(CatchUpSystems::ResetReadiness)
            .before(PredictionSystems::Rollback),
    );
    app.add_systems(
        PreUpdate,
        (
            reset_catchup_input_readiness.in_set(CatchUpSystems::ResetReadiness),
            mark_awaiting_catchup_gated_entities.before(CatchUpSystems::DetectSnapshotReady),
            finalize_catch_up_snapshot.in_set(CatchUpSystems::FinalizeSnapshot),
        )
            .run_if(initial_catchup_is_active),
    );
    app.add_systems(
        PreUpdate,
        detect_catch_up_snapshot_ready
            .in_set(CatchUpSystems::DetectSnapshotReady)
            .run_if(state_based_catchup_client_exists),
    );
    app.add_systems(
        PostUpdate,
        (panic_if_catchup_request_stalled, send_catchup_request)
            .chain()
            .run_if(initial_catchup_is_active)
            .before(MessageSystems::Send),
    );
}

fn initial_catchup_is_active(
    mode: Option<Res<CatchUpMode>>,
    manager: Option<Single<&CatchUpManager, With<Client>>>,
) -> bool {
    let Some(mode) = mode else {
        return false;
    };
    *mode != CatchUpMode::InputOnly && manager.is_some_and(|manager| !manager.completed)
}

fn state_based_catchup_client_exists(
    mode: Option<Res<CatchUpMode>>,
    manager: Option<Single<&CatchUpManager, With<Client>>>,
) -> bool {
    let Some(mode) = mode else {
        return false;
    };
    *mode != CatchUpMode::InputOnly && manager.is_some()
}

/// Client system: resets the accumulated input-safe tick before each
/// registered input type contributes its buffer coverage.
fn reset_catchup_input_readiness(mut managers: Query<&mut CatchUpManager, With<Client>>) {
    for mut manager in &mut managers {
        manager.input_checks_this_frame = 0;
        manager.input_safe_tick = Some(Tick(u32::MAX));
    }
}

/// Client system: contributes the replay-safe tick for one registered input
/// sequence type.
///
/// Before a request is accepted, this computes the minimum tick covered by the
/// local and rebroadcast buffers. After an accepted snapshot exists, it also
/// pads missing buffer prefixes so rollback replay can start at the snapshot
/// tick.
fn update_client_catchup_input_readiness<S: ActionStateSequence>(
    manager: Option<Single<&mut CatchUpManager, With<Client>>>,
    timeline: Option<Res<LocalTimeline>>,
    prediction_manager: Option<
        Single<&lightyear_prediction::prelude::PredictionManager, With<Client>>,
    >,
    mut buffers: Query<(&mut InputBuffer<S::Snapshot, S::Action>, Has<S::Marker>)>,
) {
    let Some(mut manager) = manager else {
        return;
    };
    let Some(timeline) = timeline else {
        return;
    };
    manager.input_checks_this_frame += 1;
    let local_tick = timeline.tick();
    let max_rollback_ticks = prediction_manager
        .as_ref()
        .map(|manager| manager.rollback_policy.max_rollback_ticks)
        .unwrap_or(100);

    let mut saw_buffer = false;
    let mut safe_tick = Tick(u32::MAX);
    let snapshot_tick = manager
        .pending_snapshot
        .as_ref()
        .map(|snapshot| snapshot.server_tick);
    for (mut buffer, is_local) in buffers.iter_mut() {
        if buffer.start_tick.is_none() && buffer.last_remote_tick.is_none() {
            continue;
        }
        saw_buffer = true;
        let Some(buffer_safe_tick) = (if is_local {
            buffer.end_tick()
        } else {
            buffer.last_remote_tick
        }) else {
            manager.input_safe_tick = None;
            return;
        };
        if let Some(snapshot_tick) = snapshot_tick
            && buffer_safe_tick >= snapshot_tick
        {
            pad_input_buffer_prefix::<S>(&mut buffer, snapshot_tick);
        }
        if buffer_safe_tick < safe_tick {
            safe_tick = buffer_safe_tick;
        }
    }
    if !saw_buffer || local_tick - safe_tick > i32::from(max_rollback_ticks) {
        manager.input_safe_tick = None;
        return;
    }
    if let Some(current) = manager.input_safe_tick
        && safe_tick < current
    {
        manager.input_safe_tick = Some(safe_tick);
    }
}

/// Fills missing buffer ticks before the first real input with empty inputs so
/// rollback replay can iterate from `reference_tick` without gaps.
fn pad_input_buffer_prefix<S: ActionStateSequence>(
    buffer: &mut InputBuffer<S::Snapshot, S::Action>,
    reference_tick: Tick,
) {
    let Some(start_tick) = buffer.start_tick else {
        return;
    };
    if start_tick <= reference_tick {
        return;
    }
    let Some(end_tick) = buffer.end_tick() else {
        return;
    };
    buffer.extend_to_range(reference_tick, end_tick);
    let mut tick = reference_tick;
    while tick < start_tick {
        buffer.set_empty(tick);
        tick = tick + 1;
    }
}

/// Client observer: stores the replicated catch-up snapshot metadata sent by
/// the server. The event is re-triggered locally only after the corresponding
/// Replicon checkpoint is confirmed.
fn receive_catch_up_snapshot_ready(
    trigger: On<RemoteEvent<CatchUpSnapshotReady>>,
    manager: Option<Single<&mut CatchUpManager, With<Client>>>,
) {
    let Some(mut manager) = manager else {
        return;
    };
    if manager.completed {
        return;
    }
    let event = &trigger.event().trigger;
    debug!(
        ?event.server_tick,
        ?event.replicon_tick,
        "received replicated CatchUpSnapshotReady"
    );
    manager.pending_snapshot = Some(PendingCatchUpSnapshot {
        server_tick: event.server_tick,
        replicon_tick: event.replicon_tick,
    });
    manager.suppress_checksums = true;
}

/// Client system: marks newly replicated gated entities as participating in
/// the initial catch-up rollback.
fn mark_awaiting_catchup_gated_entities(
    mode: Res<CatchUpMode>,
    manager: Option<Single<&mut CatchUpManager, With<Client>>>,
    gated: Query<Entity, (With<CatchUpGated>, Without<AwaitingCatchUpSnapshot>)>,
    mut commands: Commands,
) {
    let Some(mut manager) = manager else {
        return;
    };
    if *mode == CatchUpMode::InputOnly || manager.completed {
        return;
    }
    let mut marked_any = false;
    for entity in &gated {
        commands.entity(entity).insert(AwaitingCatchUpSnapshot);
        marked_any = true;
    }
    if marked_any {
        manager.suppress_checksums = true;
    }
}

/// Client system: emits [`CatchUpSnapshotReady`] after the accepted catch-up
/// reveal checkpoint has been completely processed.
pub(crate) fn detect_catch_up_snapshot_ready(
    mode: Res<CatchUpMode>,
    manager: Option<Single<&mut CatchUpManager, With<Client>>>,
    server_mutate_ticks: Res<ServerMutateTicks>,
    checkpoints: Res<ReplicationCheckpointMap>,
    gated: Query<(), With<CatchUpGated>>,
    mut commands: Commands,
) {
    if *mode == CatchUpMode::InputOnly {
        return;
    }
    let Some(mut manager) = manager else {
        return;
    };
    if manager.completed {
        if gated.is_empty() {
            return;
        }
        let replicon_tick = server_mutate_ticks
            .last_confirmed_tick()
            .unwrap_or_else(|| server_mutate_ticks.last_tick());
        if manager.last_emitted_replicon_tick == Some(replicon_tick) {
            return;
        }
        if !server_mutate_ticks.contains(replicon_tick) {
            return;
        }
        let Some(server_tick) = checkpoints.get(replicon_tick) else {
            return;
        };
        manager.last_emitted_replicon_tick = Some(replicon_tick);
        commands.trigger(CatchUpSnapshotReady {
            replicon_tick,
            server_tick,
        });
        return;
    }

    let Some(snapshot) = manager.pending_snapshot.as_ref() else {
        return;
    };
    let snapshot_replicon_tick = snapshot.replicon_tick;
    let snapshot_server_tick = snapshot.server_tick;
    if manager.last_emitted_replicon_tick == Some(snapshot_replicon_tick) {
        return;
    }
    if !server_mutate_ticks.contains(snapshot_replicon_tick) {
        return;
    }
    manager.last_emitted_replicon_tick = Some(snapshot_replicon_tick);
    commands.trigger(CatchUpSnapshotReady {
        replicon_tick: snapshot_replicon_tick,
        server_tick: snapshot_server_tick,
    });
}

/// Client system: after user snapshot hooks have run, requests the forced
/// rollback and clears the internal pending markers.
fn finalize_catch_up_snapshot(
    mode: Res<CatchUpMode>,
    manager: Option<Single<&mut CatchUpManager, With<Client>>>,
    mut state_metadata: Option<ResMut<StateRollbackMetadata>>,
    awaiting: Query<Entity, With<AwaitingCatchUpSnapshot>>,
    mut commands: Commands,
) {
    if *mode == CatchUpMode::InputOnly {
        return;
    }
    let Some(mut manager) = manager else {
        return;
    };
    let Some(snapshot) = manager.pending_snapshot.as_ref() else {
        return;
    };
    let Some(input_safe_tick) = catch_up_input_safe_tick(&manager) else {
        return;
    };
    if input_safe_tick < snapshot.server_tick {
        return;
    }
    let awaiting_entities = awaiting.iter().collect::<Vec<_>>();
    if awaiting_entities.is_empty() {
        manager.completed = true;
        manager.pending_snapshot = None;
        manager.request_sent_at_tick = None;
        manager.request_input_safe_tick = None;
        manager.suppress_checksums = false;
        return;
    }
    let Some(state_metadata) = state_metadata.as_deref_mut() else {
        return;
    };
    debug!(
        ?snapshot.server_tick,
        "requesting bundled forced rollback to catch-up tick"
    );
    state_metadata.request_forced_rollback(snapshot.server_tick);
    for entity in awaiting_entities {
        commands.entity(entity).remove::<AwaitingCatchUpSnapshot>();
    }
    manager.completed = true;
    manager.pending_snapshot = None;
    manager.request_sent_at_tick = None;
    manager.request_input_safe_tick = None;
    manager.suppress_checksums = false;
}

/// Client system: send a [`CatchUpRequest`] once the input timeline is
/// synced, at least one entity is waiting for a catch-up snapshot, and the
/// registered input buffers cover a safe replay tick.
///
/// Requests are sent over Lightyear's reliable metadata channel; the server
/// responds with a replicated [`CatchUpSnapshotReady`] event on the same
/// channel.
fn send_catchup_request(
    mode: Res<CatchUpMode>,
    timeline: Res<LocalTimeline>,
    awaiting: Query<(), With<AwaitingCatchUpSnapshot>>,
    client: Option<
        Single<
            (
                Entity,
                &mut MessageSender<CatchUpRequest>,
                &mut CatchUpManager,
            ),
            (With<Client>, With<IsSynced<InputTimeline>>),
        >,
    >,
) {
    if *mode == CatchUpMode::InputOnly {
        return;
    }
    if awaiting.is_empty() {
        return;
    }
    let Some(client) = client else {
        return;
    };
    let (client_entity, mut sender, mut manager) = client.into_inner();
    if manager.completed || manager.pending_snapshot.is_some() {
        return;
    };
    let Some(input_safe_tick) = catch_up_input_safe_tick(&manager) else {
        return;
    };
    if manager
        .request_input_safe_tick
        .is_some_and(|previous_tick| previous_tick >= input_safe_tick)
    {
        return;
    }
    debug!(
        ?client_entity,
        ?input_safe_tick,
        "sending CatchUpRequest to server"
    );
    sender.send::<MetadataChannel>(CatchUpRequest { input_safe_tick });
    manager.request_sent_at_tick = Some(timeline.tick());
    manager.request_input_safe_tick = Some(input_safe_tick);
    manager.suppress_checksums = true;
}

/// Returns the currently computed input-safe tick once at least one
/// registered input type has contributed this frame.
fn catch_up_input_safe_tick(manager: &CatchUpManager) -> Option<Tick> {
    if manager.input_checks_this_frame == 0 {
        return None;
    }
    manager.input_safe_tick
}

/// Client system: fails loudly if an accepted catch-up snapshot never becomes
/// ready for rollback.
pub(crate) fn panic_if_catchup_request_stalled(
    mode: Res<CatchUpMode>,
    timeout: Res<CatchUpClientTimeout>,
    timeline: Res<LocalTimeline>,
    tick_duration: Res<TickDuration>,
    client: Option<Single<(Entity, &CatchUpManager), With<Client>>>,
    awaiting: Query<
        (Entity, Option<&ConfirmHistory>, Has<CatchUpGated>),
        With<AwaitingCatchUpSnapshot>,
    >,
) {
    if *mode == CatchUpMode::InputOnly || awaiting.is_empty() {
        return;
    }
    let Some(client) = client else {
        return;
    };
    let (client_entity, manager) = client.into_inner();
    let Some(sent_at_tick) = manager.request_sent_at_tick else {
        return;
    };
    let now = timeline.tick();
    let elapsed_ticks = now - sent_at_tick;
    if elapsed_ticks <= 0 {
        return;
    }
    let elapsed = tick_duration.0.mul_f32(elapsed_ticks as f32);
    if elapsed > timeout.duration {
        panic!(
            "client {client_entity:?} requested deterministic catch-up at tick {sent_at_tick:?}, \
             but still has {} AwaitingCatchUpSnapshot entities after {:?}: {:?}; \
             catchup_manager={:?} (timeout {:?})",
            awaiting.iter().count(),
            elapsed,
            awaiting
                .iter()
                .map(|(entity, confirm, gated)| {
                    (entity, confirm.map(ConfirmHistory::last_tick), gated)
                })
                .collect::<Vec<_>>(),
            *manager,
            timeout.duration
        );
    }
}
