use crate::mode::CatchUpMode;
use bevy_app::{App, PreUpdate};
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
use bevy_replicon::prelude::RepliconTick;
use core::time::Duration;
use lightyear_connection::client::{Client, Disconnect};
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::Rollback;
use lightyear_inputs::client::InputSystems;
use lightyear_messages::prelude::{MessageSender, RemoteEvent};
use lightyear_prediction::prelude::{
    LastConfirmedInput, PredictionManager, PredictionSystems, RollbackSystems,
    StateRollbackMetadata,
};
use lightyear_prediction::rollback::{CatchUpGated, DisableRollback};
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::prelude::ReplicationSystems;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use tracing::{debug, info, warn};

use super::{CatchUpRequest, CatchUpSnapshotReady, CatchUpSystems};

/// Client-side timeout for an in-flight catch-up request.
///
/// This is intentionally a hard panic by default. A client that requested a
/// state-based catch-up but never activates it is running from stale state and
/// should fail loudly instead of silently producing misleading checksums.
#[derive(Resource, Clone, Copy, Debug)]
pub struct CatchUpClientTimeout {
    pub duration: Duration,
}

const CATCH_UP_REQUEST_RETRY_TICKS: i32 = 16;
const CATCH_UP_MAX_REQUESTS: u8 = 10;

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
#[derive(Component, Debug, Default)]
pub struct CatchUpManager {
    pub(crate) completed: bool,
    pub(crate) pending_snapshot: Option<PendingCatchUpSnapshot>,
    /// Snapshot whose forced rollback has been requested and is waiting for
    /// local activation after [`RollbackSystems::Prepare`] restores the
    /// snapshot state.
    pub(crate) activating_snapshot: Option<PendingCatchUpSnapshot>,
    pub(crate) requests_sent: u8,
    pub(crate) request_sent_at_tick: Option<Tick>,
    pub(crate) suppress_checksums: bool,
}

impl CatchUpManager {
    /// Returns true while the client is running with intentionally stale
    /// deterministic state and checksum sends should be suppressed.
    pub fn suppresses_checksums(&self) -> bool {
        self.suppress_checksums
    }
}

pub(crate) fn build(app: &mut App) {
    app.init_resource::<CatchUpClientTimeout>();
    app.register_required_components::<Client, CatchUpManager>();
    app.add_observer(on_receive_catchup_gated);
    app.add_observer(receive_catch_up_snapshot_ready);
    app.configure_sets(
        PreUpdate,
        (
            CatchUpSystems::SendCatchUpRequest,
            CatchUpSystems::TriggerCatchUpRollback.after(InputSystems::ReceiveInputMessages),
        )
            .run_if(initial_catchup_is_active)
            .after(ReplicationSystems::Receive)
            .before(PredictionSystems::Rollback),
    );
    app.configure_sets(
        PreUpdate,
        CatchUpSystems::ActivateCatchUp
            .run_if(catchup_snapshot_is_activating)
            .after(RollbackSystems::Prepare)
            .before(RollbackSystems::Rollback),
    );
    app.add_systems(
        PreUpdate,
        (
            send_catchup_request.in_set(CatchUpSystems::SendCatchUpRequest),
            trigger_snapshot_rollback.in_set(CatchUpSystems::TriggerCatchUpRollback),
        ),
    );
    app.add_systems(
        PreUpdate,
        // We first run RollbackSystems::Prepare when the catchup rollback is triggered
        // and then we trigger `CatchUpSnapshotReady`. This is to only notify the user
        // after the components have been restored to the snapshot state, but before the rollback replay starts.
        (
            trigger_catch_up_snapshot_activation,
            finish_catch_up_snapshot_activation,
        )
            .chain()
            .in_set(CatchUpSystems::ActivateCatchUp),
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

fn catchup_snapshot_is_activating(manager: Option<Single<&CatchUpManager, With<Client>>>) -> bool {
    manager.is_some_and(|manager| !manager.completed && manager.activating_snapshot.is_some())
}

/// Client system: sends a [`CatchUpRequest`] once replicated input state is
/// synced and the input plugin has confirmed a rollback-safe tick across
/// remote clients.
///
/// [`LastConfirmedInput`] is updated by the input plugin from all registered
/// remote input buffers. We use the previous frame's value here; being one
/// frame conservative is preferable to duplicating that input coverage logic.
pub(crate) fn send_catchup_request(
    timeline: Res<LocalTimeline>,
    client: Single<
        (
            Entity,
            &mut CatchUpManager,
            &LastConfirmedInput,
            &mut MessageSender<CatchUpRequest>,
            Has<IsSynced<InputTimeline>>,
        ),
        With<Client>,
    >,
    awaiting: Query<Entity, With<CatchUpGated>>,
) {
    let (client_entity, mut manager, last_confirmed_input, mut sender, is_synced) =
        client.into_inner();
    if !is_synced {
        return;
    }
    if manager.completed {
        return;
    }
    let local_tick = timeline.tick();
    if !last_confirmed_input.received_for_all_clients {
        return;
    }
    let Some(input_safe_tick) = last_confirmed_input.get() else {
        return;
    };
    if awaiting.is_empty()
        || manager.pending_snapshot.is_some()
        || manager.activating_snapshot.is_some()
    {
        return;
    }
    if manager
        .request_sent_at_tick
        .is_some_and(|sent_at_tick| local_tick - sent_at_tick < CATCH_UP_REQUEST_RETRY_TICKS)
    {
        return;
    }
    debug!(
        ?client_entity,
        ?input_safe_tick,
        "sending CatchUpRequest to server"
    );
    sender.send::<MetadataChannel>(CatchUpRequest { input_safe_tick });
    manager.requests_sent += 1;
    if manager.requests_sent > CATCH_UP_MAX_REQUESTS {
        panic!(
            "client {client_entity:?} has sent {} CatchUpRequests but still has no pending snapshot; \
             this likely means the server is failing to respond to the request; \
             check server logs for errors and verify that the server is configured to accept catch-up requests",
            manager.requests_sent
        );
    }
    manager.request_sent_at_tick = Some(local_tick);
    manager.suppress_checksums = true;
}

/// Client observer: stores the replicated catch-up snapshot metadata sent by
/// the server. The event is re-triggered locally only after the corresponding
/// Replicon checkpoint is confirmed.
fn receive_catch_up_snapshot_ready(
    trigger: On<RemoteEvent<CatchUpSnapshotReady>>,
    mut manager: Single<&mut CatchUpManager, With<Client>>,
    gated: Query<Entity, With<CatchUpGated>>,
    mut commands: Commands,
) {
    if manager.completed {
        return;
    }
    let event = &trigger.event().trigger;
    debug!(
        ?event.server_tick,
        ?event.replicon_tick,
        "received replicated CatchUpSnapshotReady"
    );
    if event.is_not_required() {
        debug!("server reported catch-up is not required");
        commands.trigger(event.clone());
        complete_catch_up(&mut manager, &gated, &mut commands);
        return;
    }
    if manager
        .pending_snapshot
        .as_mut()
        .is_none_or(|pending| pending.server_tick < event.server_tick)
    {
        manager.pending_snapshot = Some(PendingCatchUpSnapshot {
            server_tick: event.server_tick,
            replicon_tick: event.replicon_tick,
        });
    }
}

/// Client system: on receiving any CatchUpGated component, suppress checksums while
/// we wait to complete the catchup process
fn on_receive_catchup_gated(
    add: On<Add, CatchUpGated>,
    timeline: Res<LocalTimeline>,
    mut manager: Single<&mut CatchUpManager, With<Client>>,
    mut commands: Commands,
) {
    if !manager.completed {
        manager.suppress_checksums = true;
    } else {
        let tick = timeline.tick();
        commands.trigger(CatchUpSnapshotReady {
            replicon_tick: RepliconTick::new(tick.0),
            server_tick: tick,
        });
        commands.entity(add.entity).remove::<CatchUpGated>();
    }
}

/// Client system: trigger the catchup rollback after we confirm the snapshot tick has been
/// fully receive (by checking ServerMutateTicks)
pub(crate) fn trigger_snapshot_rollback(
    timeline: Res<LocalTimeline>,
    manager: Single<
        (
            Entity,
            &mut CatchUpManager,
            &LastConfirmedInput,
            &PredictionManager,
        ),
        With<Client>,
    >,
    server_mutate_ticks: Res<ServerMutateTicks>,
    mut state_metadata: ResMut<StateRollbackMetadata>,
    mut commands: Commands,
) {
    let (client_entity, mut manager, last_confirmed_input, prediction_manager) =
        manager.into_inner();
    if manager.completed {
        return;
    }
    if manager.activating_snapshot.is_some() {
        return;
    }
    let Some(snapshot) = manager.pending_snapshot.clone() else {
        return;
    };
    let snapshot_replicon_tick = snapshot.replicon_tick;
    let snapshot_server_tick = snapshot.server_tick;
    let local_tick = timeline.tick();
    let rollback_delta = local_tick - snapshot_server_tick;
    if rollback_delta < 0 {
        return;
    }
    let max_rollback_ticks = i32::from(prediction_manager.rollback_policy.max_rollback_ticks);
    if rollback_delta > max_rollback_ticks {
        warn!(
            ?client_entity,
            ?local_tick,
            ?snapshot_server_tick,
            ?snapshot_replicon_tick,
            rollback_delta,
            max_rollback_ticks,
            "disconnecting client because deterministic catch-up snapshot is too old"
        );
        commands.trigger(Disconnect {
            entity: client_entity,
        });
        return;
    }
    if !last_confirmed_input.received_for_all_clients {
        return;
    }
    // No remote input buffers (last_confirmed_input is not set) means there are no remote inputs to wait for.
    // The synced local timeline is safe to use as the catch-up coverage tick.
    let input_safe_tick = last_confirmed_input.get().unwrap_or(local_tick);
    if input_safe_tick < snapshot_server_tick {
        return;
    }
    if !server_mutate_ticks.contains(snapshot_replicon_tick) {
        return;
    }
    state_metadata.request_forced_rollback(snapshot_server_tick);
    state_metadata.clear_mismatch_history();
    manager.pending_snapshot = None;
    manager.activating_snapshot = Some(snapshot);
    info!("Triggering catchup rollback since snapshot tick: {snapshot_server_tick:?}");
}

/// Trigger local activation observers after rollback preparation has restored
/// the snapshot components, but before rollback replay starts.
/// This is so that when CatchUpSnapshotReady is observed, the CatchUpGated components are already restored
/// to their snapshot state.
fn trigger_catch_up_snapshot_activation(
    client: Query<(&Rollback, &CatchUpManager), With<Client>>,
    mut commands: Commands,
) {
    let Ok((rollback, manager)) = client.single() else {
        return;
    };
    if manager.completed || !matches!(*rollback, Rollback::FromState) {
        return;
    }
    let Some(snapshot) = manager.activating_snapshot.clone() else {
        return;
    };

    commands.trigger(CatchUpSnapshotReady {
        replicon_tick: snapshot.replicon_tick,
        server_tick: snapshot.server_tick,
    });
}

fn finish_catch_up_snapshot_activation(
    mut client: Query<(&mut CatchUpManager, &mut PredictionManager), With<Client>>,
    gated: Query<Entity, With<CatchUpGated>>,
    mut commands: Commands,
) {
    let Ok((mut manager, mut prediction_manager)) = client.single_mut() else {
        return;
    };
    if manager.completed || manager.activating_snapshot.is_none() {
        return;
    }

    for (_, entity) in prediction_manager.deterministic_skip_despawn.drain(..) {
        commands.entity(entity).try_remove::<DisableRollback>();
    }

    complete_catch_up(&mut manager, &gated, &mut commands);
}

fn complete_catch_up(
    manager: &mut CatchUpManager,
    gated: &Query<Entity, With<CatchUpGated>>,
    commands: &mut Commands,
) {
    for entity in gated.iter() {
        commands.entity(entity).remove::<CatchUpGated>();
    }
    manager.completed = true;
    manager.pending_snapshot = None;
    manager.requests_sent = 0;
    manager.request_sent_at_tick = None;
    manager.activating_snapshot = None;
    manager.suppress_checksums = false;
}
