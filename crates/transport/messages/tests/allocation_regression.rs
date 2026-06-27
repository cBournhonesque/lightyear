#![cfg(feature = "test_utils")]

use std::alloc::System;

use lightyear_messages::test_utils::{MessageQueueFixture, MessageSerializationFixture};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const MESSAGES_PER_BATCH: usize = 8;
const WARMUP_MESSAGES: usize = 100;
const MEASURED_MESSAGES: usize = 1_000;

#[test]
fn message_loop_allocation_budget() {
    let mut queue_fixture = MessageQueueFixture::default();
    let mut serialization_fixture = MessageSerializationFixture::default();

    let queue_warmup = queue_fixture.run_messages(WARMUP_MESSAGES, MESSAGES_PER_BATCH);
    assert_eq!(queue_warmup.messages, WARMUP_MESSAGES);
    let serialization_warmup = serialization_fixture.run_messages(WARMUP_MESSAGES).unwrap();
    assert_eq!(serialization_warmup.messages, WARMUP_MESSAGES);

    let region = Region::new(GLOBAL);
    let queue_stats = queue_fixture.run_messages(MEASURED_MESSAGES, MESSAGES_PER_BATCH);
    let queue_allocation_stats = region.change();

    assert_eq!(queue_stats.messages, MEASURED_MESSAGES);
    eprintln!("message queue loop allocation stats: {queue_allocation_stats:#?}");
    assert_eq!(
        queue_allocation_stats.allocations, 0,
        "message queue loop allocation count regressed: {queue_allocation_stats:#?}"
    );
    assert_eq!(
        queue_allocation_stats.reallocations, 0,
        "message queue loop reallocation count regressed: {queue_allocation_stats:#?}"
    );
    assert_eq!(
        queue_allocation_stats.bytes_allocated, 0,
        "message queue loop allocated-byte budget regressed: {queue_allocation_stats:#?}"
    );

    let region = Region::new(GLOBAL);
    let serialization_stats = serialization_fixture
        .run_messages(MEASURED_MESSAGES)
        .unwrap();
    let serialization_allocation_stats = region.change();

    assert_eq!(serialization_stats.messages, MEASURED_MESSAGES);
    eprintln!("message serialization loop allocation stats: {serialization_allocation_stats:#?}");
    assert_eq!(
        serialization_allocation_stats.allocations, 0,
        "message serialization loop allocation count regressed: {serialization_allocation_stats:#?}"
    );
    assert_eq!(
        serialization_allocation_stats.reallocations, 0,
        "message serialization loop reallocation count regressed: {serialization_allocation_stats:#?}"
    );
    assert_eq!(
        serialization_allocation_stats.bytes_allocated, 0,
        "message serialization loop allocated-byte budget regressed: {serialization_allocation_stats:#?}"
    );
}
