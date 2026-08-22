use super::trigger_state_rollback;
use crate::protocol::{CompBundleA, CompBundleB, CompFull};
use crate::stepper::{ClientServerStepper, StepperConfig};
use bevy::prelude::*;
use lightyear::frame_interpolation::FrameInterpolationHistory;
use lightyear_core::prelude::Tick;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::prelude::{Predicted, VisualCorrection};
use test_log::test;

fn replay_full(mut components: Query<&mut CompFull, With<Predicted>>) {
    for mut component in &mut components {
        *component = CompFull(10.0);
    }
}

fn replay_bundle(mut components: Query<(&mut CompBundleA, &mut CompBundleB), With<Predicted>>) {
    for (mut a, mut b) in &mut components {
        *a = CompBundleA(10.0);
        *b = CompBundleB(20.0);
    }
}

fn history<C: Component + Clone>(tick: Tick, value: C) -> PredictionHistory<C> {
    let mut history = PredictionHistory::default();
    history.add_predicted(tick, Some(value));
    history
}

/// `.predict()` installs frame-history repair in the real rollback schedule even when visual
/// correction is not enabled for the component.
#[test]
fn prediction_registration_repairs_frame_history_after_rollback() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.client_app().add_systems(FixedUpdate, replay_full);

    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 1;
    let entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            CompFull(100.0),
            history(rollback_tick, CompFull(4.0)),
            FrameInterpolationHistory::<CompFull> {
                previous_value: Some(CompFull(200.0)),
                current_value: Some(CompFull(300.0)),
            },
        ))
        .id();

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);

    let frame_history = stepper
        .client_app()
        .world()
        .get::<FrameInterpolationHistory<CompFull>>(entity)
        .unwrap();
    assert_eq!(frame_history.previous_value, Some(CompFull(4.0)));
    assert_eq!(frame_history.current_value, Some(CompFull(10.0)));
}

/// Post-rollback correction uses the bundle interpolation rule for every corrected member and
/// restores the replayed simulation values after sampling it.
#[test]
fn post_rollback_correction_uses_bundle_interpolation_rule() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.client_app().add_systems(FixedUpdate, replay_bundle);

    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 1;
    let entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            CompBundleA(1.0),
            CompBundleB(2.0),
            history(rollback_tick, CompBundleA(0.0)),
            history(rollback_tick, CompBundleB(0.0)),
            FrameInterpolationHistory::<CompBundleA>::default(),
            FrameInterpolationHistory::<CompBundleB>::default(),
        ))
        .id();

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);

    let world = stepper.client_app().world();
    let t = world.resource::<Time<Fixed>>().overstep_fraction();
    let corrected_a = 100.0 + 10.0 * t + 20.0 * t;
    let corrected_b = 200.0 + 20.0 * t;
    assert_eq!(world.get::<CompBundleA>(entity), Some(&CompBundleA(10.0)));
    assert_eq!(world.get::<CompBundleB>(entity), Some(&CompBundleB(20.0)));
    assert_eq!(
        world
            .get::<VisualCorrection<CompBundleA>>(entity)
            .map(|correction| &correction.error),
        Some(&CompBundleA(1.0 - corrected_a))
    );
    assert_eq!(
        world
            .get::<VisualCorrection<CompBundleB>>(entity)
            .map(|correction| &correction.error),
        Some(&CompBundleB(2.0 - corrected_b))
    );
}

/// A bundle member reinserted by rollback contributes its repaired prediction samples to another
/// member's correction even though it had no pre-rollback visual value of its own.
#[test]
fn post_rollback_bundle_uses_member_without_previous_visual() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.client_app().add_systems(FixedUpdate, replay_bundle);

    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 1;
    let entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            CompBundleA(1.0),
            history(rollback_tick, CompBundleA(0.0)),
            history(rollback_tick, CompBundleB(4.0)),
            FrameInterpolationHistory::<CompBundleA>::default(),
            FrameInterpolationHistory::<CompBundleB>::default(),
        ))
        .id();

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);

    let world = stepper.client_app().world();
    let t = world.resource::<Time<Fixed>>().overstep_fraction();
    let corrected_a = 100.0 + 10.0 * t + 4.0 + (20.0 - 4.0) * t;
    assert_eq!(world.get::<CompBundleA>(entity), Some(&CompBundleA(10.0)));
    assert_eq!(world.get::<CompBundleB>(entity), Some(&CompBundleB(20.0)));
    assert_eq!(
        world
            .get::<VisualCorrection<CompBundleA>>(entity)
            .map(|correction| &correction.error),
        Some(&CompBundleA(1.0 - corrected_a))
    );
    assert!(
        world.get::<VisualCorrection<CompBundleB>>(entity).is_none(),
        "the reinserted bundle member had no pre-rollback visual to correct"
    );
}
