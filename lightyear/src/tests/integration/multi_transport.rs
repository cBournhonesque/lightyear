//! Tests related to the server using multiple transports at the same time to connect to clients
use crate::client::sync::SyncConfig;
use crate::prelude::client::{InterpolationConfig, PredictionConfig};
use crate::prelude::{SharedConfig, TickConfig};
use crate::tests::multi_stepper::MultiBevyStepper;
use bevy::prelude::*;
use core::time::Duration;

#[test]
fn test_multi_transport() {
    let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        tick: TickConfig::new(tick_duration),
        ..Default::default()
    };
    let mut stepper = MultiBevyStepper::new(
        shared_config,
        SyncConfig::default(),
        PredictionConfig::default(),
        InterpolationConfig::default(),
        frame_duration,
    );
    stepper.build();
    stepper.init();

    stepper.frame_step();
    stepper.frame_step();
    // since the clients are synced, the ClientMetadata entities should be replicated already
    // let client_metadata_1 = stepper
    //     .client_app_1
    //     .world()    //     .query::<&ClientMetadata>()
    //     .get_single(&stepper.client_app_1.world_mut());
    // dbg!(client_metadata_1);

    // // spawn an entity on the server
    // stepper
    //     .server_app
    //     .world_mut()
    //     .spawn((Component1(1.0), Replicate::default()));
    // stepper.frame_step();
    // stepper.frame_step();

    // check that the entity got replicated to both clients
    // (even though they share the same client id)
}
