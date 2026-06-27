use bevy_ecs::component::Component;
use core::hint::black_box;
use lightyear_core::prelude::{ConfirmedHistory, Tick};
use lightyear_interpolation::prelude::InterpolationRegistry;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const WARMUP_MESSAGES: usize = 100;
const MEASURED_MESSAGES: usize = 1_000;

#[derive(Component, Clone, Debug, PartialEq)]
struct TestComponent(f32);

fn interpolate_component(start: TestComponent, end: TestComponent, t: f32) -> TestComponent {
    TestComponent(start.0 + (end.0 - start.0) * t)
}

fn run_interpolation_history_loop(
    registry: &InterpolationRegistry,
    history: &mut ConfirmedHistory<TestComponent>,
    next_tick: &mut u32,
    message_count: usize,
) -> usize {
    let mut messages = 0;

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
            black_box(interpolated);
        }

        messages += 1;
        *next_tick += 1;
    }

    messages
}

#[test]
#[ignore = "manual heap profile; writes target/dhat-interpolation-history-loop.json"]
fn dhat_interpolation_history_loop() {
    let mut registry = InterpolationRegistry::default();
    registry.set_interpolation::<TestComponent>(interpolate_component);
    let mut history = ConfirmedHistory::<TestComponent>::default();
    let mut next_tick = 0;

    run_interpolation_history_loop(&registry, &mut history, &mut next_tick, WARMUP_MESSAGES);

    let profile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/dhat-interpolation-history-loop.json");
    std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();

    let messages =
        run_interpolation_history_loop(&registry, &mut history, &mut next_tick, MEASURED_MESSAGES);
    assert_eq!(messages, MEASURED_MESSAGES);
}
