use bevy::prelude::*;
use lightyear::prelude::{NetworkTarget, Replicate};
use lightyear_tests::protocol::CompFull;
use lightyear_tests::stepper::ClientServerStepper;
use std::fs::File;

const N: usize = 100;

const NUM_ENTITIES: usize = 1000;
fn main() {
    for _ in 0..N {
        let mut stepper = ClientServerStepper::single();
        let entities =
            vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All)); NUM_ENTITIES];
        stepper.server_app.world_mut().spawn_batch(entities);

        // advance time by one frame
        stepper.advance_time(stepper.frame_duration);

        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(10000)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .unwrap();

        // buffer and send replication messages
        stepper.server_app.update();
        stepper.client_app().update();

        if let Ok(report) = guard.report().build() {
            let file = File::create("flamegraph.svg").unwrap();
            report.flamegraph(file).unwrap();
        };
    }
}
