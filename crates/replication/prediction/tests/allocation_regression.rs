use std::alloc::System;

use bevy_ecs::component::Component;
use core::hint::black_box;
use lightyear_core::tick::Tick;
use lightyear_prediction::prelude::PredictionHistory;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const HISTORY_WINDOW: u32 = 64;
const WARMUP_MESSAGES: usize = 100;
const MEASURED_MESSAGES: usize = 1_000;

#[derive(Component, Clone, Debug, PartialEq)]
struct TestComponent(u32);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct HistoryLoopStats {
    messages: usize,
    checksum: u64,
}

fn run_prediction_history_loop(
    history: &mut PredictionHistory<TestComponent>,
    next_tick: &mut u32,
    message_count: usize,
) -> HistoryLoopStats {
    let mut stats = HistoryLoopStats::default();

    for _ in 0..message_count {
        let tick = Tick(*next_tick);
        history.add_predicted(tick, Some(TestComponent(*next_tick)));
        if *next_tick >= HISTORY_WINDOW {
            history.clear_until_tick(Tick(*next_tick - HISTORY_WINDOW));
        }

        stats.messages += 1;
        stats.checksum ^= history.len() as u64;
        black_box(history.get(tick));

        *next_tick += 1;
    }

    stats
}

#[test]
fn prediction_history_loop_allocation_budget() {
    let mut history = PredictionHistory::<TestComponent>::default();
    let mut next_tick = 0;

    let warmup_stats = run_prediction_history_loop(&mut history, &mut next_tick, WARMUP_MESSAGES);
    assert_eq!(warmup_stats.messages, WARMUP_MESSAGES);

    let region = Region::new(GLOBAL);
    let stats = run_prediction_history_loop(&mut history, &mut next_tick, MEASURED_MESSAGES);
    let allocation_stats = region.change();

    assert_eq!(stats.messages, MEASURED_MESSAGES);
    eprintln!("prediction history loop allocation stats: {allocation_stats:#?}");
    assert_eq!(
        allocation_stats.allocations, 0,
        "prediction history loop allocation count regressed: {allocation_stats:#?}"
    );
    assert_eq!(
        allocation_stats.reallocations, 0,
        "prediction history loop reallocation count regressed: {allocation_stats:#?}"
    );
    assert_eq!(
        allocation_stats.bytes_allocated, 0,
        "prediction history loop allocated-byte budget regressed: {allocation_stats:#?}"
    );
}
