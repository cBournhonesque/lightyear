//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use bevy::log::tracing_subscriber::fmt::format::FmtSpan;
use bevy::log::{info, tracing_subscriber};
use bevy::prelude::{default, error, Events};
use bevy::utils::tracing;
use bevy::utils::tracing::Level;
use bevy::utils::Duration;
use divan::{AllocProfiler, Bencher};
use lightyear::client::sync::SyncConfig;
use lightyear::prelude::client::{
    ClientConnection, InterpolationConfig, NetClient, PredictionConfig,
};
use lightyear::prelude::server::Replicate;
use lightyear::prelude::{client, server, MessageRegistry, Tick, TickManager};
use lightyear::prelude::{ClientId, SharedConfig, TickConfig};
use lightyear::server::input::native::InputBuffers;
use lightyear::shared::replication::network_target::NetworkTarget;
use lightyear_benches::local_stepper::{LocalBevyStepper, Step as LocalStep};
use lightyear_benches::protocol::*;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

criterion_group!(
    replication_benches,
    replicate_simple_component_to_one_client,
    replicate_simple_component_to_multiple_clients
);
criterion_main!(replication_benches);

const NUM_ENTITIES: &[usize] = &[0, 10, 100, 1000, 10000];

/// Replicating N entity spawn from server to channel, with a local io
fn replicate_simple_component_to_one_client(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("replication/replicate_simple_message_to_one_client");
    group.warm_up_time(std::time::Duration::from_millis(500));
    group.measurement_time(std::time::Duration::from_millis(6000));
    for n in NUM_ENTITIES.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_entities", n),
            n,
            |bencher, n| {
                bencher.iter_batched_ref(
                    || {
                        let mut stepper = LocalBevyStepper::default();
                        let entities = vec![(Component1(0.0), Replicate::default()); *n];
                        stepper.server_app.world.spawn_batch(entities);
                        stepper
                    },
                    |stepper| {
                        stepper.frame_step();
                        // let client_id = ClientId::Netcode(0);
                        // assert_eq!(
                        //     stepper
                        //         .client_apps
                        //         .get(&client_id)
                        //         .unwrap()
                        //         .world
                        //         .entities()
                        //         .len(),
                        //     n as u32
                        // );
                    },
                    BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

const FIXED_NUM_ENTITIES: usize = 10;
const NUM_CLIENTS: &[usize] = &[0, 1, 2, 4, 8, 16];

/// Replicating entity spawns from server to N clients, with a socket io
fn replicate_simple_component_to_multiple_clients(criterion: &mut Criterion) {
    let mut group =
        criterion.benchmark_group("replication/replicate_simple_component_to_multiple_client");
    group.warm_up_time(std::time::Duration::from_millis(500));
    group.measurement_time(std::time::Duration::from_millis(6000));
    for n in NUM_CLIENTS.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_clients", n),
            n,
            |bencher, n| {
                bencher.iter_batched_ref(
                    || {
                        let mut stepper = LocalBevyStepper::default_n_clients(*n);
                        let entities =
                            vec![(Component1(0.0), Replicate::default()); FIXED_NUM_ENTITIES];
                        stepper.server_app.world.spawn_batch(entities);
                        stepper
                    },
                    |stepper| {
                        stepper.frame_step();
                        // for i in 0..*n {
                        //     let client_id = ClientId::Netcode(i as u64);
                        //     assert_eq!(
                        //         stepper
                        //             .client_apps
                        //             .get(&client_id)
                        //             .unwrap()
                        //             .world
                        //             .entities()
                        //             .len(),
                        //         FIXED_NUM_ENTITIES as u32
                        //     );
                        // }
                    },
                    BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}
