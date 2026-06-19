use bevy_app::{App, PreUpdate};
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
use bevy_replicon::prelude::RepliconTick;
use core::time::Duration;
use lightyear_connection::client::Client;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::{Tick};
use lightyear_messages::prelude::{MessageSender, RemoteEvent};
use lightyear_prediction::prelude::{
    LastConfirmedInput, PredictionSystems, StateRollbackMetadata,
};
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::prelude::{ReplicationSystems};
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use tracing::{debug, info};
use lightyear_prediction::rollback::CatchUpGated;
use crate::mode::CatchUpMode;

use super::{
    CatchUpRequest, CatchUpSnapshotReady, CatchUpSystems,
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

const CATCH_UP_REQUEST_RETRY_TICKS: i32 = 8;
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
#[derive(Component, Debug)]
pub struct CatchUpManager {
    pub(crate) completed: bool,
    pub(crate) pending_snapshot: Option<PendingCatchUpSnapshot>,
    pub(crate) requests_sent: u8,
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
            requests_sent: 0,
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


pub(crate) fn build(app: &mut App) {
    app.init_resource::<CatchUpClientTimeout>();
    app.register_required_components::<Client, CatchUpManager>();
    app.add_observer(on_receive_catchup_gated);
    app.add_observer(receive_catch_up_snapshot_ready);
    app.configure_sets(
        PreUpdate,
        (
            CatchUpSystems::SendCatchUpRequest,
            CatchUpSystems::TriggerCatchUpRollback,
        )
            .run_if(initial_catchup_is_active)
            .after(ReplicationSystems::Receive)
            .before(PredictionSystems::Rollback),
    );
    app.add_systems(
        PreUpdate,
        (
            send_catchup_request.in_set(CatchUpSystems::SendCatchUpRequest),
            trigger_snapshot_rollback.in_set(CatchUpSystems::TriggerCatchUpRollback),
        )
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
    awaiting: Query<(), With<CatchUpGated>>,
) {
    let (
        client_entity,
        mut manager,
        last_confirmed_input,
        mut sender,
        is_synced,
    ) = client.into_inner();
    if !is_synced {
        return;
    }
    let local_tick = timeline.tick();
    let Some(mut input_safe_tick) = last_confirmed_input.get() else {
        return;
    };
    if awaiting.is_empty() || manager.pending_snapshot.is_some() {
        return;
    }
    if manager
        .request_sent_at_tick
        .is_some_and(|sent_at_tick| local_tick - sent_at_tick < CATCH_UP_REQUEST_RETRY_TICKS) {
        return;
    }
    info!(?manager.request_input_safe_tick, ?input_safe_tick, "checking if we can send");
    // keep using the earliest input_safe_tick we have recorded
    if let Some(previous_input_safe_tick) = manager.request_input_safe_tick {
        input_safe_tick = core::cmp::min(input_safe_tick, previous_input_safe_tick);
    }
    debug!(
        ?client_entity,
        ?input_safe_tick,
        previous_input_safe_tick = ?manager.request_input_safe_tick,
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
    manager.request_input_safe_tick = Some(input_safe_tick);
    manager.suppress_checksums = true;
}

/// Client observer: stores the replicated catch-up snapshot metadata sent by
/// the server. The event is re-triggered locally only after the corresponding
/// Replicon checkpoint is confirmed.
fn receive_catch_up_snapshot_ready(
    trigger: On<RemoteEvent<CatchUpSnapshotReady>>,
    mut manager: Single<&mut CatchUpManager, With<Client>>,
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
    if manager.pending_snapshot.as_mut().is_none_or(|pending| pending.server_tick < event.server_tick) {
        manager.pending_snapshot = Some(PendingCatchUpSnapshot {
            server_tick: event.server_tick,
            replicon_tick: event.replicon_tick,
        });
    }
}

/// Client system: on receiving any CatchUpGated component, suppress checksums while
/// we wait to complete the catchup process
fn on_receive_catchup_gated(
    _: On<Add, CatchUpGated>,
    mut manager: Single<&mut CatchUpManager, With<Client>>,
) {
    if !manager.completed {
        manager.suppress_checksums = true;
    }
}

/// Client system: trigger the catchup rollback after we confirm the snapshot tick has been
/// fully receive (by checking ServerMutateTicks)
pub(crate) fn trigger_snapshot_rollback(
    mut manager: Single<&mut CatchUpManager, With<Client>>,
    server_mutate_ticks: Res<ServerMutateTicks>,
    mut state_metadata: ResMut<StateRollbackMetadata>,
    gated: Query<Entity, With<CatchUpGated>>,
    mut commands: Commands,
) {
    if manager.completed {
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
    state_metadata.request_forced_rollback(snapshot_server_tick);
    info!("Triggering catchup rollback since snapshot tick: {snapshot_server_tick:?}");
    for entity in gated.iter() {
        commands.entity(entity).remove::<CatchUpGated>();
    }
    manager.completed = true;
    manager.pending_snapshot = None;
    manager.requests_sent = 0;
    manager.request_sent_at_tick = None;
    manager.request_input_safe_tick = None;
    manager.suppress_checksums = false;
}