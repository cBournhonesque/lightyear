#![cfg(feature = "test_utils")]

use std::alloc::System;

use lightyear_transport::packet::test_utils::PacketLoopFixture;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const MESSAGES_PER_BATCH: usize = 4;
const PAYLOAD_BYTES: usize = 64;
const WARMUP_MESSAGES: usize = 1_500;
const MEASURED_MESSAGES: usize = 1_000;

#[test]
fn packet_send_receive_loop_allocation_budget() {
    let mut fixture = PacketLoopFixture::new(MESSAGES_PER_BATCH, PAYLOAD_BYTES);

    let warmup = fixture.prepare_batches(WARMUP_MESSAGES);
    let warmup_stats = fixture.run_batches(warmup).unwrap();
    assert_eq!(
        warmup_stats.packets,
        fixture.expected_packets_for_messages(WARMUP_MESSAGES)
    );
    assert_eq!(warmup_stats.messages, WARMUP_MESSAGES);
    assert_eq!(
        warmup_stats.payload_bytes,
        fixture.expected_payload_bytes_for_messages(WARMUP_MESSAGES)
    );

    let batches = fixture.prepare_batches(MEASURED_MESSAGES);
    let region = Region::new(GLOBAL);
    let stats = fixture.run_batches(batches).unwrap();
    let allocation_stats = region.change();

    assert_eq!(
        stats.packets,
        fixture.expected_packets_for_messages(MEASURED_MESSAGES)
    );
    assert_eq!(stats.messages, MEASURED_MESSAGES);
    assert_eq!(
        stats.payload_bytes,
        fixture.expected_payload_bytes_for_messages(MEASURED_MESSAGES)
    );

    eprintln!("packet loop allocation stats: {allocation_stats:#?}");

    assert_eq!(
        allocation_stats.allocations, 0,
        "packet loop allocation count regressed: {allocation_stats:#?}"
    );
    assert_eq!(
        allocation_stats.reallocations, 0,
        "packet loop reallocation count regressed: {allocation_stats:#?}"
    );
    assert_eq!(
        allocation_stats.bytes_allocated, 0,
        "packet loop allocated-byte budget regressed: {allocation_stats:#?}"
    );
}
