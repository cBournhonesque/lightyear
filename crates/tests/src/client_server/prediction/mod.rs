use crate::stepper::*;
use bevy::prelude::*;
use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
use bevy_replicon::prelude::RepliconTick;
use lightyear_core::prelude::{Rollback, Tick};
use lightyear_prediction::prelude::*;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;

mod correction;
mod despawn;
mod history;
mod prespawn;
mod rollback;
mod spawn;

fn record_completed_mutate_tick_for_rollback_check(world: &mut World, tick: Tick) {
    let replicon_tick = RepliconTick::new(tick.0);
    world
        .resource_mut::<ServerMutateTicks>()
        .confirm(replicon_tick, 1);
    let mut checkpoints = world.resource_mut::<ReplicationCheckpointMap>();
    checkpoints.record(replicon_tick, tick);
    checkpoints.record_last_confirmed_checkpoint(replicon_tick);
}

/// Helper for tests that exercise rollback restoration/replay rather than mismatch detection.
/// Records a completed checkpoint and explicitly requests the one-shot rollback.
pub(crate) fn request_forced_rollback_at_completed_tick(
    stepper: &mut ClientServerStepper,
    tick: Tick,
) {
    let world = stepper.client_app().world_mut();
    record_completed_mutate_tick_for_rollback_check(world, tick);
    world
        .resource_mut::<StateRollbackMetadata>()
        .request_forced_rollback(tick);
}

pub(crate) fn trigger_state_rollback(stepper: &mut ClientServerStepper, tick: Tick) {
    stepper.client_app().insert_resource(Rollback::FromState);
    stepper
        .client_app()
        .world()
        .resource::<PredictionManager>()
        .set_rollback_tick(tick);
}
