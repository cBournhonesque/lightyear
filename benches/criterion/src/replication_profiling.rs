use bevy::prelude::*;
use lightyear::prelude::{NetworkTarget, Replicate};
use lightyear_tests::protocol::CompFull;
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};
use std::fs::File;

const NUM_FRAMES: usize = 100;
const N: usize = 10;

const NUM_ENTITIES: usize = 1000;
fn main() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(N));
    let components = vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All)); NUM_ENTITIES];
    let entities = stepper
        .server_app
        .world_mut()
        .spawn_batch(components)
        .collect::<Vec<_>>();

    // advance time by one frame
    stepper.advance_time(stepper.frame_duration);

    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(10000)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .unwrap();

    for _ in 0..NUM_FRAMES {
        stepper.frame_step_server_first(1);

        // update the component on the server
        entities.iter().for_each(|entity| {
            stepper
                .server_app
                .world_mut()
                .get_mut::<CompFull>(*entity)
                .unwrap()
                .0 += 1.0;
        });
    }

    if let Ok(report) = guard.report().build() {
        let file = File::create("flamegraph.svg").unwrap();
        report.flamegraph(file).unwrap();
    };
}
