//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use bevy::prelude::With;
use core::time::Duration;
use lightyear::prelude::{NetworkTarget, Replicate, Replicating};
use lightyear_benches::profiler::FlamegraphProfiler;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use lightyear_tests::protocol::CompFull;
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};

criterion_group!(
    name = replication_benches;
    config = Criterion::default().with_profiler(FlamegraphProfiler::new(3000));
    targets = send_float_insert_one_client,
    send_float_update_one_client,
    receive_float_insert,
    receive_float_update,
    send_float_insert_n_clients,
);
criterion_main!(replication_benches);

// const NUM_ENTITIES: &[usize] = &[0, 10, 100, 1000, 10000];
const NUM_ENTITIES: &[usize] = &[1000];

/// Replicating N entity spawn from server to channel, with a local io
fn send_float_insert_one_client(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("replication/send_float_insert/1_client");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_millis(4000));
    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
                        let entities =
                            vec![(CompFull(0.0), Replicate::to_clients(NetworkTarget::All),); *n];
                        stepper.server_app.world_mut().spawn_batch(entities);

                        // advance time by one frame
                        stepper.advance_time(stepper.frame_duration);

                        let instant = Instant::now();
                        // buffer and send replication messages
                        stepper.server_app.update();
                        elapsed += instant.elapsed();

                        stepper.client_app().update();
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}

/// Replicating N entity spawn from server to channel, with a local io
fn send_float_update_one_client(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("replication/send_float_update/1_client");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_millis(4000));
    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
                        let entities =
                            vec![(CompFull(1.0), Replicate::to_clients(NetworkTarget::All)); *n];
                        stepper.server_app.world_mut().spawn_batch(entities);
                        stepper.frame_step(2);

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
                        stepper.advance_time(stepper.frame_duration);

                        let instant = Instant::now();
                        // buffer and send replication messages
                        stepper.server_app.update();
                        elapsed += instant.elapsed();

                        stepper.client_app().update();
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}

/// Receiving N float component inserts, with a local io
fn receive_float_insert(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("replication/receive_float_insert/1_client");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_millis(4000));
    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
                        let entities =
                            vec![(CompFull(1.0), Replicate::to_clients(NetworkTarget::All)); *n];
                        stepper.server_app.world_mut().spawn_batch(entities);

                        stepper.advance_time(stepper.frame_duration);
                        stepper.server_app.update();

                        // receive messages
                        let instant = Instant::now();
                        stepper.client_app().update();
                        elapsed += instant.elapsed();
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}

/// Replicating N entity spawn from server to channel, with a local io
fn receive_float_update(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("replication/receive_float_update/1_client");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_millis(4000));
    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
                        let entities =
                            vec![(CompFull(1.0), Replicate::to_clients(NetworkTarget::All)); *n];
                        stepper.server_app.world_mut().spawn_batch(entities);
                        stepper.frame_step(2);

                        // update the entities
                        for mut component in stepper
                            .server_app
                            .world_mut()
                            .query_filtered::<&mut CompFull, With<Replicating>>()
                            .iter_mut(stepper.server_app.world_mut())
                        {
                            component.0 = 0.0;
                        }

                        stepper.advance_time(stepper.frame_duration);
                        stepper.server_app.update();
                        let instant = Instant::now();
                        stepper.client_app().update();
                        elapsed += instant.elapsed();
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}

const FIXED_NUM_ENTITIES: usize = 1000;
const NUM_CLIENTS: &[usize] = &[0, 1, 2, 4, 8, 16];

/// Replicating entity spawns from server to N clients, with a socket io
fn send_float_insert_n_clients(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("replication/send_float_inserts/n_clients");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_millis(4000));
    for n in NUM_CLIENTS.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = ClientServerStepper::from_config(
                            StepperConfig::with_netcode_clients(*n),
                        );
                        let entities =
                            vec![(CompFull(0.0), Replicate::default()); FIXED_NUM_ENTITIES];
                        stepper.server_app.world_mut().spawn_batch(entities);

                        // advance time by one frame
                        stepper.advance_time(stepper.frame_duration);

                        let instant = Instant::now();
                        // buffer and send replication messages
                        stepper.server_app.update();
                        elapsed += instant.elapsed();

                        stepper.client_app().update();
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}
