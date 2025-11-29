//! Benchmark to measure the performance of replicating Entity spawns
#![allow(unused_imports)]

use lightyear::prelude::MessageSender;
use lightyear_tests::protocol::{Channel1, StringMessage};
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};

use divan::{AllocProfiler, Bencher};

#[global_allocator]
static ALLOC: AllocProfiler = AllocProfiler::system();

const NUM_MESSAGE: &[usize] = &[1, 10, 100, 1000];
const MESSAGE_LEN: &[usize] = &[1, 10, 100, 1000];

#[divan::bench(
    args = NUM_MESSAGE,
    consts = MESSAGE_LEN,
)]
/// Sending N message from server to channel, with a local io
fn send_receive_simple_messages_to_one_client<const N: usize>(bencher: Bencher, num_message: usize) {
    bencher
        .with_inputs(|| ClientServerStepper::from_config(StepperConfig::single()))
        .bench_values(|mut stepper| {
            for _ in 0..num_message {
            stepper
                    .client_of_mut(0)
                    .get_mut::<MessageSender<StringMessage>>()
                    .unwrap()
                    .send::<Channel1>(StringMessage(['a'; N].iter().collect()));
            }
            stepper.frame_step(1);
        });
}

// TODO: send_receive_long_message_to_one_client
// TODO: send_receive_random_message_to_one_client (with fuzzing)
// TODO: send_receive_simple_message_to_many_clients

fn main() {
    divan::main();
}