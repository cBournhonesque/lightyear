//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use bevy::prelude::With;
use core::time::Duration;
use lightyear::prelude::{GLOBAL_RECORDER, MetricsRegistry, NetworkTarget, Replicate, Replicating};
use std::ops::Deref;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use lightyear::metrics::metrics;
use lightyear_benches::measurements::bandwidth::{Bandwidth, BandwidthChannel};
use lightyear_tests::protocol::CompFull;
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};

criterion_group!(
    name = replication_bandwidth;
    config = Criterion::default().with_measurement(Bandwidth);
    targets = send_float_insert_one_client,
    send_float_update_one_client,
);

// const NUM_ENTITIES: &[usize] = &[0, 10, 100, 1000, 10000];
const NUM_ENTITIES: &[usize] = &[1000];

/// Replicating N entity spawn from server to channel, with a local io
fn send_float_insert_one_client(criterion: &mut Criterion<Bandwidth>) {
    let mut group = criterion.benchmark_group("replication/send_float_insert/1_client");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(10));
    group.measurement_time(Duration::from_millis(10));

    fn f(num_entities: usize) -> f64 {
        let mut config = StepperConfig::single();
        config.server_registry = Some(GLOBAL_RECORDER.clone());
        let mut stepper = ClientServerStepper::from_config(config);
        let start = Bandwidth::value(
            GLOBAL_RECORDER.deref(),
            true,
            false,
            BandwidthChannel::Replication,
        );
        let entities =
            vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All),); num_entities];
        stepper.server_app.world_mut().spawn_batch(entities);
        stepper.frame_step_server_first(1);
        let end = Bandwidth::value(
            GLOBAL_RECORDER.deref(),
            true,
            false,
            BandwidthChannel::Replication,
        );
        end - start
    }

    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut total = 0.0;
                    for _ in 0..iter {
                        total += std::hint::black_box(f(*n));
                        #[cfg(test)]
                        dbg!("replication/send_float_insert/1_client", total);
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

/// Replicating N entity spawn from server to channel, with a local io
fn send_float_update_one_client(criterion: &mut Criterion<Bandwidth>) {
    let mut group = criterion.benchmark_group("replication/send_float_update/1_client");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(10));
    group.measurement_time(Duration::from_millis(10));

    fn f(num_entities: usize) -> f64 {
        let mut config = StepperConfig::single();
        config.server_registry = Some(GLOBAL_RECORDER.clone());
        let mut stepper = ClientServerStepper::from_config(config);
        let entities =
            vec![(CompFull(1.0), Replicate::to_clients(NetworkTarget::All)); num_entities];
        stepper.server_app.world_mut().spawn_batch(entities);
        stepper.frame_step(2);

        let start = Bandwidth::value(
            GLOBAL_RECORDER.deref(),
            true,
            false,
            BandwidthChannel::Replication,
        );

        // update the entities
        for mut component in stepper
            .server_app
            .world_mut()
            .query_filtered::<&mut CompFull, With<Replicating>>()
            .iter_mut(stepper.server_app.world_mut())
        {
            component.0 = 0.0;
        }

        // advance time by one frame
        stepper.frame_step_server_first(1);
        let end = Bandwidth::value(
            GLOBAL_RECORDER.deref(),
            true,
            false,
            BandwidthChannel::Replication,
        );
        end - start
    }

    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut total = 0.0;
                    for _ in 0..iter {
                        total += std::hint::black_box(f(*n));
                        #[cfg(test)]
                        dbg!("replication/send_float_update/1_client", total);
                    }
                    total
                });
            },
        );
    }
    group.finish();
}
