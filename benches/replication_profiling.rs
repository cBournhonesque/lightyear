use bevy::prelude::*;
use lightyear::prelude::server::Replicate;
use lightyear::prelude::ReplicationGroup;
use lightyear_benches::local_stepper::{LocalBevyStepper, Step};
use lightyear_benches::protocol::Component1;
use std::fs::File;

const N: usize = 100;

const NUM_ENTITIES: usize = 1000;
fn main() {
    for _ in 0..N {
        let mut stepper = LocalBevyStepper::default();
        let entities = vec![
            (
                Component1(0.0),
                Replicate {
                    group: ReplicationGroup::new_id(1),
                    ..default()
                }
            );
            NUM_ENTITIES
        ];
        stepper.server_app.world_mut().spawn_batch(entities);

        // advance time by one frame
        stepper.advance_time(stepper.frame_duration);

        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(10000)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .unwrap();

        // buffer and send replication messages
        stepper.server_update();
        stepper.client_update();

        if let Ok(report) = guard.report().build() {
            let file = File::create("flamegraph.svg").unwrap();
            report.flamegraph(file).unwrap();
        };
    }
}
