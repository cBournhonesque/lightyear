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
use lightyear::prelude::client::{InterpolationConfig, PredictionConfig};
use lightyear::prelude::{client, server, MessageRegistry, Tick, TickManager};
use lightyear::prelude::{ClientId, NetworkTarget, SharedConfig, TickConfig};
use lightyear::server::input::InputBuffers;
use lightyear_benches::local_stepper::{LocalBevyStepper, Step as LocalStep};
use lightyear_benches::protocol::*;

fn main() {
    divan::main()
}

// #[global_allocator]
// static ALLOC: AllocProfiler = AllocProfiler::system();

const NUM_ENTITIES: &[usize] = &[0, 10, 100, 1000, 10000];
// const NUM_CLIENTS: &[usize] = &[0, 1, 2, 4, 8, 16];

/// Replicating N entity spawn from server to channel, with a local io
#[divan::bench(
    sample_count = 1,
    args = NUM_ENTITIES,
)]
fn send_message(bencher: Bencher, n: usize) {
    tracing_subscriber::FmtSubscriber::builder()
        .with_span_events(FmtSpan::ENTER)
        .with_max_level(tracing::Level::INFO)
        .init();

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

            // let entities = vec![
            //     (
            //         Component1(0.0),
            //         Replicate {
            //             replication_target: NetworkTarget::All,
            //             ..default()
            //         },
            //     );
            //     n
            // ];

            // stepper.server_app.world.spawn_batch(entities);
            stepper
        })
        .bench_values(|mut stepper| {
            let client_id = ClientId::Netcode(0);
            let _ = stepper
                .server_app
                .world
                .resource_mut::<server::ConnectionManager>()
                .send_message::<Channel1, _>(client_id, Message2(1))
                .inspect_err(|e| error!("error: {e:?}"));
            stepper.frame_step();
            let client_id = ClientId::Netcode(0);
            assert_eq!(
                stepper
                    .client_apps
                    .get_mut(&client_id)
                    .unwrap()
                    .world
                    .resource_mut::<Events<client::MessageEvent<Message2>>>()
                    .drain()
                    .map(|e| e.message().clone())
                    .collect::<Vec<_>>(),
                vec![Message2(1)]
            );

            let _ = stepper
                .client_apps
                .get_mut(&client_id)
                .unwrap()
                .world
                .resource_mut::<client::ConnectionManager>()
                .send_message::<Channel1, _>(Message2(3))
                .inspect_err(|e| error!("error: {e:?}"));
            stepper.frame_step();
            stepper.frame_step();
            assert_eq!(
                stepper
                    .server_app
                    .world
                    .resource_mut::<Events<server::MessageEvent<Message2>>>()
                    .drain()
                    .map(|e| e.message().clone())
                    .collect::<Vec<_>>(),
                vec![Message2(3)]
            );

            // assert_eq!(stepper.client_app.world.entities().len(), n as u32);
            // dbg!(stepper.client().io().stats());
        });
}

// /// Replicating N entity spawn from server to channel, with a local io
// #[divan::bench(
//     sample_count = 100,
//     args = NUM_ENTITIES,
// )]
// fn spawn_local(bencher: Bencher, n: usize) {
//     bencher
//         .with_inputs(|| {
//             let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
//             let tick_duration = Duration::from_millis(10);
//             let shared_config = SharedConfig {
//                 tick: TickConfig::new(tick_duration),
//                 ..default()
//             };
//             let mut stepper = LocalBevyStepper::new(
//                 1,
//                 shared_config,
//                 SyncConfig::default(),
//                 PredictionConfig::default(),
//                 InterpolationConfig::default(),
//                 frame_duration,
//             );
//             stepper.init();
//
//             let entities = vec![
//                 (
//                     Component1(0.0),
//                     Replicate {
//                         replication_target: NetworkTarget::All,
//                         ..default()
//                     },
//                 );
//                 n
//             ];
//
//             stepper.server_app.world.spawn_batch(entities);
//             stepper
//         })
//         .bench_values(|mut stepper| {
//             stepper.frame_step();
//             stepper.frame_step();
//
//             let client_id = ClientId::Netcode(0);
//             assert_eq!(
//                 stepper
//                     .client_apps
//                     .get(&client_id)
//                     .unwrap()
//                     .world
//                     .entities()
//                     .len(),
//                 1 + n as u32
//             );
//             // assert_eq!(stepper.client_app.world.entities().len(), n as u32);
//             // dbg!(stepper.client().io().stats());
//         });
// }

// const FIXED_NUM_ENTITIES: usize = 10;

// /// Replicating entity spawns from server to N clients, with a socket io
// #[divan::bench(
//     sample_count = 100,
//     args = NUM_CLIENTS,
// )]
// fn spawn_multi_clients(bencher: Bencher, n: usize) {
//     bencher
//         .with_inputs(|| {
//             let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
//             let tick_duration = Duration::from_millis(10);
//             let shared_config = SharedConfig {
//                 tick: TickConfig::new(tick_duration),
//                 ..default()
//             };
//             let mut stepper = LocalBevyStepper::new(
//                 n,
//                 shared_config,
//                 SyncConfig::default(),
//                 PredictionConfig::default(),
//                 InterpolationConfig::default(),
//                 frame_duration,
//             );
//             stepper.init();
//
//             let entities = vec![
//                 (
//                     Component1(0.0),
//                     Replicate {
//                         replication_target: NetworkTarget::All,
//                         ..default()
//                     },
//                 );
//                 FIXED_NUM_ENTITIES
//             ];
//
//             stepper.server_app.world.spawn_batch(entities);
//             stepper
//         })
//         .bench_values(|mut stepper| {
//             stepper.frame_step();
//             stepper.frame_step();
//
//             for i in 0..n {
//                 let client_id = ClientId::Netcode(i as u64);
//                 assert_eq!(
//                     stepper
//                         .client_apps
//                         .get(&client_id)
//                         .unwrap()
//                         .world
//                         .entities()
//                         .len(),
//                     1 + FIXED_NUM_ENTITIES as u32
//                 );
//             }
//             // dbg!(stepper.client().io().stats());
//         });
// }
