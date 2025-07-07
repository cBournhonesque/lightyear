use crate::stepper::ClientServerStepper;
use bevy::prelude::{Entity, Event, EventReader, Events, Query, Single, With};
use lightyear_core::prelude::{Rollback, Tick};
use lightyear_prediction::prelude::PredictionManager;
use lightyear_replication::components::Confirmed;
use lightyear_replication::prelude::ReplicationReceiver;

mod pre_prediction;

mod correction;
mod despawn;
mod history;
mod prespawn;
mod rollback;
mod spawn;

/// Mock that we received an update for the Confirmed entity at a given tick
#[derive(Event)]
pub(crate) struct RollbackInfo {
    tick: Tick,
}

/// Helper function to simulate that we received a server message and trigger a rollback check.
/// We have to add a system because otherwise the ReplicationReceiver resets `set_received_this_frame`
/// in ReplicationSet::Receive
pub(crate) fn trigger_rollback_system(
    mut events: EventReader<RollbackInfo>,
    mut receiver: Single<&mut ReplicationReceiver, With<PredictionManager>>,
    mut query: Query<&mut Confirmed>,
) {
    for event in events.read() {
        receiver.set_received_this_frame();
        for mut confirmed in query.iter_mut() {
            confirmed.tick = event.tick;
        }
    }
}

pub(crate) fn trigger_rollback_check(stepper: &mut ClientServerStepper, tick: Tick) {
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<Events<RollbackInfo>>()
        .send(RollbackInfo { tick });
}

pub(crate) fn trigger_rollback(stepper: &mut ClientServerStepper, tick: Tick) {
    stepper.client_mut(0).insert(Rollback::FromState);
    stepper
        .client_mut(0)
        .get_mut::<PredictionManager>()
        .unwrap()
        .set_rollback_tick(tick);
    stepper
        .client_app()
        .world_mut()
        .query::<&mut Confirmed>()
        .iter_mut(stepper.client_app().world_mut())
        .for_each(|mut confirmed| {
            confirmed.tick = tick;
        })
}
