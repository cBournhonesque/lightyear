use bevy_ecs::component::Component;
use core::hint::black_box;
use lightyear_core::tick::Tick;
use lightyear_prediction::prelude::PredictionHistory;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const HISTORY_WINDOW: u32 = 64;
const WARMUP_MESSAGES: usize = 100;
const MEASURED_MESSAGES: usize = 1_000;

#[derive(Component, Clone, Debug, PartialEq)]
struct TestComponent(u32);

fn run_prediction_history_loop(
    history: &mut PredictionHistory<TestComponent>,
    next_tick: &mut u32,
    message_count: usize,
) -> usize {
    let mut messages = 0;

    for _ in 0..message_count {
        let tick = Tick(*next_tick);
        history.add_predicted(tick, Some(TestComponent(*next_tick)));
        if *next_tick >= HISTORY_WINDOW {
            history.clear_until_tick(Tick(*next_tick - HISTORY_WINDOW));
        }

        messages += 1;
        black_box(history.get(tick));

        *next_tick += 1;
    }

    messages
}

#[test]
#[ignore = "manual heap profile; writes target/dhat-prediction-history-loop.json"]
fn dhat_prediction_history_loop() {
    let mut history = PredictionHistory::<TestComponent>::default();
    let mut next_tick = 0;

    run_prediction_history_loop(&mut history, &mut next_tick, WARMUP_MESSAGES);

    let profile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/dhat-prediction-history-loop.json");
    std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();

    let messages = run_prediction_history_loop(&mut history, &mut next_tick, MEASURED_MESSAGES);
    assert_eq!(messages, MEASURED_MESSAGES);
}
