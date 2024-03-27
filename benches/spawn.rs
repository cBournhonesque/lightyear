//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use bevy::log::info;
use bevy::prelude::default;
use bevy::utils::tracing;
use bevy::utils::tracing::Level;
use bevy::utils::Duration;
use divan::{AllocProfiler, Bencher};
use lightyear::client::sync::SyncConfig;
use lightyear::prelude::client::{InterpolationConfig, PredictionConfig};
use lightyear::prelude::{ClientId, NetworkTarget, SharedConfig, TickConfig};
use lightyear_benches::local_stepper::{LocalBevyStepper, Step as LocalStep};
use lightyear_benches::protocol::*;

fn main() {
    divan::main()
}

// #[global_allocator]
// static ALLOC: AllocProfiler = AllocProfiler::system();

const NUM_ENTITIES: &[usize] = &[0, 10, 100, 1000, 10000];
const NUM_CLIENTS: &[usize] = &[0, 1, 2, 4, 8, 16];

/// Replicating N entity spawn from server to channel, with a local io
#[divan::bench(
    sample_count = 100,
    args = NUM_ENTITIES,
)]
fn spawn_local(bencher: Bencher, n: usize) {
    bencher
        .with_inputs(|| {
            let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
            let tick_duration = Duration::from_millis(10);
            let shared_config = SharedConfig {
                tick: TickConfig::new(tick_duration),
                ..default()
            };
            let mut stepper = LocalBevyStepper::new(
                1,
                shared_config,
                SyncConfig::default(),
                PredictionConfig::default(),
                InterpolationConfig::default(),
                frame_duration,
            );
            stepper.init();

            let entities = vec![
                (
                    Component1(0.0),
                    Replicate {
                        replication_target: NetworkTarget::All,
                        ..default()
                    },
                );
                n
            ];

            stepper.server_app.world.spawn_batch(entities);
            stepper
        })
        .bench_values(|mut stepper| {
            stepper.frame_step();
            stepper.frame_step();

            let client_id = 0 as ClientId;
            assert_eq!(
                stepper
                    .client_apps
                    .get(&client_id)
                    .unwrap()
                    .world
                    .entities()
                    .len(),
                1 + n as u32
            );
            // assert_eq!(stepper.client_app.world.entities().len(), n as u32);
            // dbg!(stepper.client().io().stats());
        });
}

const FIXED_NUM_ENTITIES: usize = 10;

/// Replicating entity spawns from server to N clients, with a socket io
#[divan::bench(
    sample_count = 100,
    args = NUM_CLIENTS,
)]
fn spawn(bencher: Bencher, n: usize) {
    bencher
        .with_inputs(|| {
            let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
            let tick_duration = Duration::from_millis(10);
            let shared_config = SharedConfig {
                tick: TickConfig::new(tick_duration),
                ..default()
            };
            let mut stepper = LocalBevyStepper::new(
                n,
                shared_config,
                SyncConfig::default(),
                PredictionConfig::default(),
                InterpolationConfig::default(),
                frame_duration,
            );
            stepper.init();

            let entities = vec![
                (
                    Component1(0.0),
                    Replicate {
                        replication_target: NetworkTarget::All,
                        ..default()
                    },
                );
                FIXED_NUM_ENTITIES
            ];

            stepper.server_app.world.spawn_batch(entities);
            stepper
        })
        .bench_values(|mut stepper| {
            stepper.frame_step();
            stepper.frame_step();

            for i in 0..n {
                let client_id = i as ClientId;
                assert_eq!(
                    stepper
                        .client_apps
                        .get(&client_id)
                        .unwrap()
                        .world
                        .entities()
                        .len(),
                    1 + FIXED_NUM_ENTITIES as u32
                );
            }
            // dbg!(stepper.client().io().stats());
        });
}
