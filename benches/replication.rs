//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use bevy::log::tracing_subscriber::fmt::format::FmtSpan;
use bevy::log::{info, tracing_subscriber};
use bevy::prelude::{default, error, Events, With};
use bevy::utils::tracing;
use bevy::utils::tracing::Level;
use bevy::utils::Duration;
use divan::{AllocProfiler, Bencher};
use lightyear::client::sync::SyncConfig;
use lightyear::prelude::client::{
    ClientConnection, InterpolationConfig, NetClient, PredictionConfig,
};
use lightyear::prelude::server::Replicate;
use lightyear::prelude::{
    client, server, MessageRegistry, Replicating, ReplicationGroup, Tick, TickManager,
};
use lightyear::prelude::{ClientId, SharedConfig, TickConfig};
use lightyear::server::input::native::InputBuffers;
use lightyear::shared::replication::network_target::NetworkTarget;
use lightyear_benches::local_stepper::{LocalBevyStepper, Step as LocalStep};
use lightyear_benches::profiler::FlamegraphProfiler;
use lightyear_benches::protocol::*;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use lightyear::channel::builder::EntityActionsChannel;
use lightyear::server::connection::ConnectionManager;

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
                        let mut stepper = LocalBevyStepper::default();
                        let entities = vec![
                            (
                                Component1(0.0),
                                Replicate {
                                    group: ReplicationGroup::new_id(1),
                                    ..default()
                                }
                            );
                            *n
                        ];
                        stepper.server_app.world_mut().spawn_batch(entities);

                        // advance time by one frame
                        stepper.advance_time(stepper.frame_duration);

                        let instant = Instant::now();
                        // buffer and send replication messages
                        stepper.server_update();
                        elapsed += instant.elapsed();
                        // dbg!(stepper
                        //     .server_app
                        //     .world()                        //     .resource::<ConnectionManager>()
                        //     .connection(ClientId::Netcode(0))
                        //     .unwrap()
                        //     .message_manager
                        //     .channel_send_stats::<EntityActionsChannel>());

                        stepper.client_update();
                        // assert_eq!(
                        //     stepper
                        //         .client_apps
                        //         .get(&ClientId::Netcode(0))
                        //         .unwrap()
                        //         .world()                        //         .entities()
                        //         .len(),
                        //     *n as u32
                        // );
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
    group.warm_up_time(std::time::Duration::from_millis(500));
    group.measurement_time(std::time::Duration::from_millis(4000));
    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = LocalBevyStepper::default();
                        let entities = vec![(Component1(1.0), Replicate::default()); *n];
                        stepper.server_app.world_mut().spawn_batch(entities);
                        stepper.update();

                        // update the entities
                        for mut component in stepper
                            .server_app
                            .world_mut()
                            .query_filtered::<&mut Component1, With<Replicating>>()
                            .iter_mut(stepper.server_app.world_mut())
                        {
                            component.0 = 0.0;
                        }

                        // advance time by one frame
                        stepper.advance_time(stepper.frame_duration);

                        let instant = Instant::now();
                        // buffer and send replication messages
                        stepper.server_update();
                        elapsed += instant.elapsed();

                        stepper.client_update();
                        assert_eq!(
                            stepper
                                .client_apps
                                .get(&ClientId::Netcode(0))
                                .unwrap()
                                .world()
                                .entities()
                                .len(),
                            *n as u32
                        );
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
    group.warm_up_time(std::time::Duration::from_millis(500));
    group.measurement_time(std::time::Duration::from_millis(4000));
    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = LocalBevyStepper::default();
                        let entities = vec![
                            (
                                Component1(1.0),
                                Replicate {
                                    group: ReplicationGroup::new_id(1),
                                    ..default()
                                }
                            );
                            *n
                        ];
                        stepper.server_app.world_mut().spawn_batch(entities);

                        // advance time by one frame
                        stepper.advance_time(stepper.frame_duration);

                        // buffer and send replication messages
                        stepper.server_update();

                        // receive messages
                        let instant = Instant::now();
                        stepper.client_update();
                        elapsed += instant.elapsed();
                        assert_eq!(
                            stepper
                                .client_apps
                                .get(&ClientId::Netcode(0))
                                .unwrap()
                                .world()
                                .entities()
                                .len(),
                            *n as u32
                        );
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
                        let mut stepper = LocalBevyStepper::default();
                        let entities = vec![(Component1(1.0), Replicate::default()); *n];
                        stepper.server_app.world_mut().spawn_batch(entities);
                        stepper.update();

                        // update the entities
                        for mut component in stepper
                            .server_app
                            .world_mut()
                            .query_filtered::<&mut Component1, With<Replicating>>()
                            .iter_mut(stepper.server_app.world_mut())
                        {
                            component.0 = 0.0;
                        }

                        // advance time by one frame
                        stepper.advance_time(stepper.frame_duration);

                        // buffer and send replication messages
                        stepper.server_update();
                        let instant = Instant::now();
                        stepper.client_update();
                        elapsed += instant.elapsed();
                        assert_eq!(
                            stepper
                                .client_apps
                                .get(&ClientId::Netcode(0))
                                .unwrap()
                                .world()
                                .entities()
                                .len(),
                            *n as u32
                        );
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
    group.warm_up_time(std::time::Duration::from_millis(500));
    group.measurement_time(std::time::Duration::from_millis(4000));
    for n in NUM_CLIENTS.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_custom(|iter| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iter {
                        let mut stepper = LocalBevyStepper::default_n_clients(*n);
                        let entities =
                            vec![(Component1(0.0), Replicate::default()); FIXED_NUM_ENTITIES];
                        stepper.server_app.world_mut().spawn_batch(entities);

                        // advance time by one frame
                        stepper.advance_time(stepper.frame_duration);

                        let instant = Instant::now();
                        // buffer and send replication messages
                        stepper.server_update();
                        elapsed += instant.elapsed();

                        stepper.client_update();
                        for i in 0..*n {
                            assert_eq!(
                                stepper
                                    .client_apps
                                    .get(&ClientId::Netcode(i as u64))
                                    .unwrap()
                                    .world()
                                    .entities()
                                    .len(),
                                FIXED_NUM_ENTITIES as u32
                            );
                        }
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}
