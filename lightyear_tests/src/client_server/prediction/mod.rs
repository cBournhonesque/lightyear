use crate::stepper::ClientServerStepper;
use bevy::prelude::{Entity, Event, EventReader, Events, Query, Single, With};
use lightyear_core::prelude::Tick;
use lightyear_prediction::prelude::PredictionManager;
use lightyear_replication::components::Confirmed;
use lightyear_replication::prelude::ReplicationReceiver;

mod pre_prediction;

mod rollback;
mod correction;

/// Mock that we received an update for the Confirmed entity at a given tick
#[derive(Event)]
pub(crate) struct RollbackInfo {
    confirmed: Entity,
    tick: Tick,
}

/// Helper function to simulate that we received a server message
pub(crate) fn trigger_rollback_system(
    mut events: EventReader<RollbackInfo>,
    mut receiver: Single<&mut ReplicationReceiver, With<PredictionManager>>,
    mut query: Query<&mut Confirmed>
) {
    for event in events.read() {
        receiver.set_received_this_frame();
        if let Ok(mut confirmed) = query.get_mut(event.confirmed) {
            confirmed.tick = event.tick;
        }
    }
}

pub(crate) fn trigger_rollback(stepper: &mut ClientServerStepper, rollback_info: RollbackInfo) {
    stepper.client_app().world_mut().resource_mut::<Events<RollbackInfo>>().send(rollback_info);
}