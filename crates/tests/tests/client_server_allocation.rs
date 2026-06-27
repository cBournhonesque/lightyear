#![cfg(feature = "test_utils")]

use std::alloc::System;

use bevy::prelude::*;
use core::hint::black_box;
use lightyear::prelude::{MessageReceiver, MessageSender};
use lightyear_tests::protocol::{Channel1, StringMessage};
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const WARMUP_MESSAGES: usize = 100;
const MEASURED_MESSAGES: usize = 1_000;

#[derive(Resource, Default)]
struct ReceivedMessages {
    count: usize,
    checksum: usize,
}

fn drain_server_messages(
    mut receivers: Query<&mut MessageReceiver<StringMessage>>,
    mut received: ResMut<ReceivedMessages>,
) {
    for mut receiver in receivers.iter_mut() {
        for message in receiver.receive() {
            received.count += 1;
            received.checksum ^= message.0.len();
            black_box(message);
        }
    }
}

fn make_messages(prefix: &str, count: usize) -> Vec<StringMessage> {
    (0..count)
        .map(|i| StringMessage(format!("{prefix}-{i:04}")))
        .collect()
}

fn quiet_frame_step(stepper: &mut ClientServerStepper) {
    stepper.advance_time(stepper.frame_duration);
    for client_app in &mut stepper.client_apps {
        client_app.update();
    }
    stepper.server_app.update();
}

fn send_client_messages(
    stepper: &mut ClientServerStepper,
    messages: impl IntoIterator<Item = StringMessage>,
) {
    for message in messages {
        stepper
            .client_mut(0)
            .get_mut::<MessageSender<StringMessage>>()
            .unwrap()
            .send::<Channel1>(message);
        quiet_frame_step(stepper);
    }
}

#[test]
#[ignore = "manual full-stack allocation profile; noisy by design"]
fn client_server_message_loop_allocation_profile() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.server_app.init_resource::<ReceivedMessages>();
    stepper
        .server_app
        .add_systems(Update, drain_server_messages);

    send_client_messages(&mut stepper, make_messages("warmup", WARMUP_MESSAGES));
    let received_before = stepper
        .server_app
        .world()
        .resource::<ReceivedMessages>()
        .count;
    assert_eq!(received_before, WARMUP_MESSAGES);

    let messages = make_messages("measured", MEASURED_MESSAGES);
    let region = Region::new(GLOBAL);
    send_client_messages(&mut stepper, messages);
    let allocation_stats = region.change();

    let received = stepper.server_app.world().resource::<ReceivedMessages>();
    assert_eq!(received.count - received_before, MEASURED_MESSAGES);

    eprintln!("client-server message loop allocation stats: {allocation_stats:#?}");
    eprintln!(
        "client-server message loop received: count={} checksum={}",
        received.count - received_before,
        received.checksum
    );
}
