use crate::stepper::ClientServerStepper;
use bevy::prelude::*;
use lightyear_core::prelude::{Rollback, Tick};
use lightyear_prediction::prelude::*;
use lightyear_replication::prelude::{ConfirmedTick, ReplicationReceiver};

mod correction;
mod despawn;
mod history;
mod prespawn;
mod rollback;
mod spawn;

/// Mock that we received an update for the Confirmed entity at a given tick
#[derive(Message)]
pub(crate) struct RollbackInfo {
    tick: Tick,
}

/// Helper function to simulate that we received a server message and trigger a rollback check.
/// We have to add a system because otherwise the ReplicationReceiver resets `set_received_this_frame`
/// in ReplicationSet::Receive
pub(crate) fn trigger_rollback_system(
    mut events: MessageReader<RollbackInfo>,
    mut receiver: Single<&mut ReplicationReceiver, With<PredictionManager>>,
    mut query: Query<&mut ConfirmedTick, With<Predicted>>,
) {
    for event in events.read() {
        receiver.received_this_frame = true;
        for mut confirmed_tick in query.iter_mut() {
            confirmed_tick.tick = event.tick;
        }
    }
}

pub(crate) fn trigger_rollback_check(stepper: &mut ClientServerStepper, tick: Tick) {
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<Messages<RollbackInfo>>()
        .write(RollbackInfo { tick });
}

pub(crate) fn trigger_state_rollback(stepper: &mut ClientServerStepper, tick: Tick) {
    stepper.client_mut(0).insert(Rollback::FromState);
    stepper
        .client_mut(0)
        .get_mut::<PredictionManager>()
        .unwrap()
        .set_rollback_tick(tick);
    stepper
        .client_app()
        .world_mut()
        .query::<&mut ConfirmedTick>()
        .iter_mut(stepper.client_app().world_mut())
        .for_each(|mut confirmed| {
            confirmed.tick = tick;
        })
}
