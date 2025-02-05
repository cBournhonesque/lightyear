//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use bevy::log::tracing_subscriber::fmt::format::FmtSpan;
use bevy::log::{info, tracing_subscriber};
use bevy::prelude::{default, error, Events};
use bevy::utils::tracing;
use bevy::utils::tracing::Level;
use bevy::utils::Duration;
use lightyear::client::sync::SyncConfig;
use lightyear::prelude::client::{InterpolationConfig, PredictionConfig};
use lightyear::prelude::{client, server, MessageRegistry, Tick, TickManager};
use lightyear::prelude::{ClientId, SharedConfig, TickConfig};
use lightyear::server::input::native::InputBuffers;
use lightyear::shared::replication::network_target::NetworkTarget;
use lightyear_benches::local_stepper::{LocalBevyStepper, Step as LocalStep};
use lightyear_benches::protocol::*;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

criterion_group!(message_benches, send_receive_simple_messages_to_one_client);
criterion_main!(message_benches);

const NUM_MESSAGE: &[usize] = &[0, 10, 100, 1000, 10000];

/// Sending N message from server to channel, with a local io
fn send_receive_simple_messages_to_one_client(criterion: &mut Criterion) {
    let mut group =
        criterion.benchmark_group("message/send_receive_simplek w_messages_to_one_client");
    group.warm_up_time(std::time::Duration::from_millis(500));
    group.measurement_time(std::time::Duration::from_millis(3000));
    for n in NUM_MESSAGE.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_messages", n),
            n,
            |bencher, n| {
                bencher.iter_batched_ref(
                    LocalBevyStepper::default,
                    |stepper| {
                        let client_id = ClientId::Netcode(0);
                        for _ in 0..*n {
                            let _ = stepper
                                .server_app
                                .world_mut()
                                .resource_mut::<server::ConnectionManager>()
                                .send_message::<Channel1, _>(client_id, &mut Message2(1))
                                .inspect_err(|e| error!("error: {e:?}"));
                        }
                        stepper.frame_step();
                        // assert_eq!(
                        //     stepper
                        //         .client_apps
                        //         .get_mut(&client_id)
                        //         .unwrap()
                        //         .world_mut()
                        //         .resource_mut::<Events<client::MessageEvent<Message2>>>()
                        //         .drain()
                        //         .map(|e| e.message)
                        //         .collect::<Vec<_>>(),
                        //     vec![Message2(1); *n]
                        // );
                    },
                    BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

// TODO: send_receive_long_message_to_one_client
// TODO: send_receive_random_message_to_one_client (with fuzzing)
// TODO: send_receive_simple_message_to_many_clients
