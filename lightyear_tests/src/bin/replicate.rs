use lightyear_connection::network_target::NetworkTarget;
use lightyear_replication::prelude::Replicate;
use lightyear_tests::protocol::CompFull;
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};

const NUM_ENTITIES: usize = 1000;

fn main() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let entities =
        vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All),); NUM_ENTITIES];
    stepper.server_app.world_mut().spawn_batch(entities);

    stepper.advance_time(stepper.frame_duration);
    stepper.server_app.update();

    // spawn a second batch (allocations should be reused)
    let entities =
    vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All),); NUM_ENTITIES];
    stepper.server_app.world_mut().spawn_batch(entities);

    stepper.advance_time(stepper.frame_duration);
    stepper.server_app.update();
}
