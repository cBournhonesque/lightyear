use crate::stepper::*;
use bevy::prelude::*;
use bevy_replicon::client::ClientSystems;
use lightyear_core::prelude::{Rollback, Tick};
use lightyear_prediction::manager::StateRollbackMetadata;
use lightyear_prediction::prelude::*;

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

/// Helper function to simulate that we received a server message and trigger a rollback check.
/// Sets a pending rollback check that will be applied during the next frame_step,
/// after the per-frame state reset but before the rollback check.
pub(crate) fn trigger_rollback_check(stepper: &mut ClientServerStepper, tick: Tick) {
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
