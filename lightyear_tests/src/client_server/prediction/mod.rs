use crate::stepper::*;
use bevy::prelude::*;
use bevy_replicon::client::ClientSystems;
use bevy_replicon::prelude::RepliconTick;
use lightyear_core::prelude::{Rollback, Tick};
use lightyear_prediction::manager::StateRollbackMetadata;
use lightyear_prediction::prelude::*;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;

mod correction;
mod despawn;
mod history;
mod prespawn;
mod rollback;
mod spawn;

/// Resource that stores a pending rollback check tick.
/// This is consumed by `apply_pending_rollback_check` during PreUpdate,
/// after `check_received_replication_messages` resets the per-frame state
/// but before `check_rollback` reads it.
#[derive(Resource, Default)]
pub(crate) struct PendingRollbackCheck {
    pub tick: Option<Tick>,
}

/// System that applies the pending rollback check to the StateRollbackMetadata.
/// Runs after ReplicationSystems::Receive (which includes the frame state reset)
/// and before RollbackSystems::Check (which reads the metadata).
fn apply_pending_rollback_check(
    mut pending: ResMut<PendingRollbackCheck>,
    mut metadata: ResMut<StateRollbackMetadata>,
) {
    if let Some(tick) = pending.tick.take() {
        metadata.record_mismatch(tick);
    }
}

/// Register the pending rollback check resource and system on a client app.
/// Must be called before `trigger_rollback_check` is used.
///
/// The system is ordered:
/// - after `ClientSystems::Receive` (which transitively orders it after
///   `check_received_replication_messages`, ensuring the per-frame metadata
///   reset has already happened)
/// - before `RollbackSystems::Check` (which reads the metadata)
pub(crate) fn register_rollback_check_helper(app: &mut App) {
    app.init_resource::<PendingRollbackCheck>();
    app.add_systems(
        PreUpdate,
        apply_pending_rollback_check
            .after(ClientSystems::Receive)
            .before(RollbackSystems::Check),
    );
}

fn record_completed_mutate_tick_for_rollback_check(world: &mut World, tick: Tick) {
    let replicon_tick = RepliconTick::new(tick.0);
    let mut checkpoints = world.resource_mut::<ReplicationCheckpointMap>();
    checkpoints.record(replicon_tick, tick);
    checkpoints.record_last_confirmed_tick(replicon_tick);
}

/// Helper function to simulate that we received a server message and trigger a rollback check.
/// Sets a pending rollback check that will be applied during the next frame_step,
/// after the per-frame state reset but before the rollback check.
///
/// State rollbacks are only consumed once a completed mutate tick reaches the mismatch, so this
/// helper also records `tick` as completed.
pub(crate) fn trigger_rollback_check(stepper: &mut ClientServerStepper, tick: Tick) {
    record_completed_mutate_tick_for_rollback_check(stepper.client_app().world_mut(), tick);
    trigger_rollback_check_without_completed_tick(stepper, tick);
}

/// Same as [`trigger_rollback_check`], but leaves the completed mutate tick unchanged.
///
/// Use this when testing that explicit mismatches wait for global mutate completion.
pub(crate) fn trigger_rollback_check_without_completed_tick(
    stepper: &mut ClientServerStepper,
    tick: Tick,
) {
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<PendingRollbackCheck>()
        .tick = Some(tick);
}

pub(crate) fn trigger_state_rollback(stepper: &mut ClientServerStepper, tick: Tick) {
    stepper.client_mut(0).insert(Rollback::FromState);
    stepper
        .client_mut(0)
        .get_mut::<PredictionManager>()
        .unwrap()
        .set_rollback_tick(tick);
}
