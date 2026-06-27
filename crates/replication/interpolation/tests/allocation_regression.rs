use std::alloc::System;

use bevy_ecs::component::Component;
use core::hint::black_box;
use lightyear_core::prelude::{ConfirmedHistory, Tick};
use lightyear_interpolation::prelude::InterpolationRegistry;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const WARMUP_MESSAGES: usize = 100;
const MEASURED_MESSAGES: usize = 1_000;

#[derive(Component, Clone, Debug, PartialEq)]
struct TestComponent(f32);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct HistoryLoopStats {
    messages: usize,
    checksum: u64,
}

fn interpolate_component(start: TestComponent, end: TestComponent, t: f32) -> TestComponent {
    TestComponent(start.0 + (end.0 - start.0) * t)
}

fn run_interpolation_history_loop(
    registry: &InterpolationRegistry,
    history: &mut ConfirmedHistory<TestComponent>,
    next_tick: &mut u32,
    message_count: usize,
) -> HistoryLoopStats {
    let mut stats = HistoryLoopStats::default();

    for _ in 0..message_count {
        let tick = Tick(*next_tick);
        if history.is_empty() || (*next_tick).is_multiple_of(4) {
            // SAFETY: this fixture only appends monotonic ticks.
            unsafe {
                history.insert_present_assume_sorted(tick, TestComponent(*next_tick as f32));
            }
        } else {
            history.push_unchanged(tick);
        }

        let interpolation_tick = Tick(next_tick.saturating_sub(1));
        while history.len() >= 3
            && history
                .get_nth_tick(1)
                .is_some_and(|tick| tick <= interpolation_tick)
        {
            history.pop_present();
        }

        if let (Some((start_tick, start)), Some((end_tick, end))) =
            (history.get_nth_present(0), history.get_nth_present(1))
        {
            let fraction = ((interpolation_tick - start_tick) as f32
                / (end_tick - start_tick) as f32)
                .clamp(0.0, 1.0);
            let interpolated = registry.interpolate(start.clone(), end.clone(), fraction);
            stats.checksum ^= interpolated.0.to_bits() as u64;
            black_box(interpolated);
        }

        stats.messages += 1;
        stats.checksum ^= history.len() as u64;
        *next_tick += 1;
    }

    stats
}

#[test]
fn interpolation_history_loop_allocation_budget() {
    let mut registry = InterpolationRegistry::default();
    registry.set_interpolation::<TestComponent>(interpolate_component);
    let mut history = ConfirmedHistory::<TestComponent>::default();
    let mut next_tick = 0;

    let warmup_stats =
        run_interpolation_history_loop(&registry, &mut history, &mut next_tick, WARMUP_MESSAGES);
    assert_eq!(warmup_stats.messages, WARMUP_MESSAGES);

    let region = Region::new(GLOBAL);
    let stats =
        run_interpolation_history_loop(&registry, &mut history, &mut next_tick, MEASURED_MESSAGES);
    let allocation_stats = region.change();

    assert_eq!(stats.messages, MEASURED_MESSAGES);
    eprintln!("interpolation history loop allocation stats: {allocation_stats:#?}");
    assert_eq!(
        allocation_stats.allocations, 0,
        "interpolation history loop allocation count regressed: {allocation_stats:#?}"
    );
    assert_eq!(
        allocation_stats.reallocations, 0,
        "interpolation history loop reallocation count regressed: {allocation_stats:#?}"
    );
    assert_eq!(
        allocation_stats.bytes_allocated, 0,
        "interpolation history loop allocated-byte budget regressed: {allocation_stats:#?}"
    );
}
