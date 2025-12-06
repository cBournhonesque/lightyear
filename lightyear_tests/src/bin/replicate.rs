use lightyear_connection::network_target::NetworkTarget;
use lightyear_replication::prelude::Replicate;
use lightyear_tests::protocol::CompFull;
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};

const NUM_ENTITIES: usize = 1000;

fn main() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let entities = vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All),); NUM_ENTITIES];
    stepper.server_app.world_mut().spawn_batch(entities);

    stepper.advance_time(stepper.frame_duration);
    stepper.server_app.update();

    // spawn a second batch (allocations should be reused)
    let entities = vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All),); NUM_ENTITIES];
    stepper.server_app.world_mut().spawn_batch(entities);

    stepper.advance_time(stepper.frame_duration);
    stepper.server_app.update();
}

// Results: (RUST_LOG=info) 785c170275e39c60e1f588e2c25368af6dee4ea8
// 1st update
// - replicate: 270us
// - send_replication_message: 96us
// - Message::send: 12us
// - Transport::buffer_send: 15us
// - Netcode::send: 35us

// 2nd update:
// Same, but Message::send: 5us


// Results: after switching to ReplicateState (61e848e79f89e313f6455936ed99474c87217836)
// Exactly the same results.