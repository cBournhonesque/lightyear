//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use lightyear::prelude::MessageSender;
use lightyear_tests::protocol::{Channel1, StringMessage};
use lightyear_tests::stepper::ClientServerStepper;

criterion_group!(message_benches, send_receive_simple_messages_to_one_client);
criterion_main!(message_benches);

const NUM_MESSAGE: &[usize] = &[0, 10, 100, 1000, 10000];

/// Sending N message from server to channel, with a local io
fn send_receive_simple_messages_to_one_client(criterion: &mut Criterion) {
    let mut group =
        criterion.benchmark_group("message/send_receive_simplek w_messages_to_one_client");
    group.warm_up_time(core::time::Duration::from_millis(500));
    group.measurement_time(core::time::Duration::from_millis(3000));
    for n in NUM_MESSAGE.iter() {
        group.bench_with_input(
            criterion::BenchmarkId::new("num_messages", n),
            n,
            |bencher, n| {
                bencher.iter_batched_ref(
                    || ClientServerStepper::single(),
                    |stepper| {
                        for _ in 0..*n {
                            stepper
                                .client_of_mut(0)
                                .get_mut::<MessageSender<StringMessage>>()
                                .unwrap()
                                .send::<Channel1>(StringMessage("a".to_string()));
                        }
                        stepper.frame_step(1);
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
