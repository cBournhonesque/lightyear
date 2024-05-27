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
use lightyear::prelude::{ClientId, SharedConfig, TickConfig};
use lightyear::server::input::native::InputBuffers;
use lightyear::shared::replication::network_target::NetworkTarget;
use lightyear_benches::local_stepper::{LocalBevyStepper, Step as LocalStep};
use lightyear_benches::protocol::*;

fn main() {
    divan::main()
}

// #[global_allocator]
// static ALLOC: AllocProfiler = AllocProfiler::system();

const NUM_MESSAGE: &[usize] = &[0, 10, 100, 1000, 10000];

/// Sending N message from server to channel, with a local io
#[divan::bench(
    sample_count = 100,
    args = NUM_MESSAGE,
)]
fn send_message(bencher: Bencher, n: usize) {
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

            let client_id = ClientId::Netcode(0);
            for _ in 0..n {
                let _ = stepper
                    .server_app
                    .world
                    .resource_mut::<server::ConnectionManager>()
                    .send_message::<Channel1, _>(client_id, &Message2(1))
                    .inspect_err(|e| error!("error: {e:?}"));
            }
            stepper
        })
        .bench_values(|mut stepper| {
            let client_id = ClientId::Netcode(0);
            stepper.frame_step();
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
                vec![Message2(1); n]
            );
        });
}
