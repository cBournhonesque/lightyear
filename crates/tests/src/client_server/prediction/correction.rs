use super::trigger_state_rollback;
use crate::protocol::{
    CompCorrectionBundleA, CompCorrectionBundleB, CompMixedCorrectionBundleA,
    CompMixedCorrectionBundleB, CompPredictionOnly,
};
use crate::stepper::{ClientServerStepper, StepperConfig};
use avian2d::math::Vector;
use avian2d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use core::time::Duration;
use lightyear::frame_interpolation::FrameInterpolationHistory;
use lightyear_core::prelude::Tick;
use lightyear_prediction::correction::PreviousVisual;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::prelude::{Predicted, VisualCorrection};
use test_log::test;

fn replay_prediction_only(mut components: Query<&mut CompPredictionOnly, With<Predicted>>) {
    for mut component in &mut components {
        *component = CompPredictionOnly(10.0);
    }
}

fn replay_correction_bundle(
    mut components: Query<
        (&mut CompCorrectionBundleA, &mut CompCorrectionBundleB),
        With<Predicted>,
    >,
) {
    for (mut a, mut b) in &mut components {
        *a = CompCorrectionBundleA(10.0);
        *b = CompCorrectionBundleB(20.0);
    }
}

fn replay_mixed_correction_bundle(
    mut components: Query<
        (
            &mut CompMixedCorrectionBundleA,
            &mut CompMixedCorrectionBundleB,
        ),
        With<Predicted>,
    >,
) {
    for (mut a, mut b) in &mut components {
        *a = CompMixedCorrectionBundleA(10.0);
        *b = CompMixedCorrectionBundleB(20.0);
    }
}

fn replay_avian_pose(
    mut components: Query<
        (
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut AngularVelocity,
        ),
        With<Predicted>,
    >,
) {
    for (mut position, mut rotation, mut linear, mut angular) in &mut components {
        *position = Position::default();
        *rotation = Rotation::default();
        *linear = LinearVelocity::default();
        *angular = AngularVelocity::default();
    }
}

fn history<C: Component + Clone>(tick: Tick, value: C) -> PredictionHistory<C> {
    let mut history = PredictionHistory::default();
    history.add_predicted(tick, Some(value));
    history
}

fn set_correction_sampling_time(stepper: &mut ClientServerStepper) {
    let mut time = Time::<Fixed>::from_duration(Duration::from_secs(1));
    time.accumulate_overstep(Duration::from_millis(500));
    stepper.client_app().insert_resource(time);
}

/// `.predict()` installs frame-history repair in the real rollback schedule even when visual
/// correction is not enabled for the component.
#[test]
fn prediction_registration_repairs_frame_history_after_rollback() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper
        .client_app()
        .add_systems(FixedUpdate, replay_prediction_only);

    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 1;
    let entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            CompPredictionOnly(100.0),
            history(rollback_tick, CompPredictionOnly(4.0)),
            FrameInterpolationHistory::<CompPredictionOnly> {
                previous_value: Some(CompPredictionOnly(200.0)),
                current_value: Some(CompPredictionOnly(300.0)),
            },
        ))
        .id();

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);

    let frame_history = stepper
        .client_app()
        .world()
        .get::<FrameInterpolationHistory<CompPredictionOnly>>(entity)
        .unwrap();
    assert_eq!(frame_history.previous_value, Some(CompPredictionOnly(4.0)));
    assert_eq!(frame_history.current_value, Some(CompPredictionOnly(10.0)));
}

/// Post-rollback correction selects the context-aware bundle rule over competing component rules,
/// uses the fixed-step sample duration, and restores the replayed values after sampling it.
#[test]
fn post_rollback_correction_uses_bundle_interpolation_rule() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    set_correction_sampling_time(&mut stepper);
    stepper
        .client_app()
        .add_systems(FixedUpdate, replay_correction_bundle);

    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 1;
    let entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            CompCorrectionBundleA(1.0),
            CompCorrectionBundleB(2.0),
            history(rollback_tick, CompCorrectionBundleA(0.0)),
            history(rollback_tick, CompCorrectionBundleB(0.0)),
            FrameInterpolationHistory::<CompCorrectionBundleA>::default(),
            FrameInterpolationHistory::<CompCorrectionBundleB>::default(),
        ))
        .id();

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);

    let world = stepper.client_app().world();
    assert_eq!(world.resource::<Time<Fixed>>().overstep_fraction(), 0.5);
    assert_eq!(
        world.get::<CompCorrectionBundleA>(entity),
        Some(&CompCorrectionBundleA(10.0))
    );
    assert_eq!(
        world.get::<CompCorrectionBundleB>(entity),
        Some(&CompCorrectionBundleB(20.0))
    );
    assert_eq!(
        world
            .get::<VisualCorrection<CompCorrectionBundleA>>(entity)
            .map(|correction| &correction.error),
        Some(&CompCorrectionBundleA(-105.0))
    );
    assert_eq!(
        world
            .get::<VisualCorrection<CompCorrectionBundleB>>(entity)
            .map(|correction| &correction.error),
        Some(&CompCorrectionBundleB(-209.0))
    );
    assert!(
        world
            .get::<PreviousVisual<CompCorrectionBundleA>>(entity)
            .is_none()
    );
    assert!(
        world
            .get::<PreviousVisual<CompCorrectionBundleB>>(entity)
            .is_none()
    );
}

/// A bundle member registered for prediction but not correction contributes its repaired samples
/// to another member's correction without receiving correction state of its own.
#[test]
fn post_rollback_bundle_uses_member_without_previous_visual() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    set_correction_sampling_time(&mut stepper);
    stepper
        .client_app()
        .add_systems(FixedUpdate, replay_mixed_correction_bundle);

    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 1;
    let entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            CompMixedCorrectionBundleA(1.0),
            CompMixedCorrectionBundleB(2.0),
            history(rollback_tick, CompMixedCorrectionBundleA(0.0)),
            history(rollback_tick, CompMixedCorrectionBundleB(4.0)),
            FrameInterpolationHistory::<CompMixedCorrectionBundleA>::default(),
            FrameInterpolationHistory::<CompMixedCorrectionBundleB>::default(),
        ))
        .id();

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);

    let world = stepper.client_app().world();
    assert_eq!(world.resource::<Time<Fixed>>().overstep_fraction(), 0.5);
    assert_eq!(
        world.get::<CompMixedCorrectionBundleA>(entity),
        Some(&CompMixedCorrectionBundleA(10.0))
    );
    assert_eq!(
        world.get::<CompMixedCorrectionBundleB>(entity),
        Some(&CompMixedCorrectionBundleB(20.0))
    );
    assert_eq!(
        world
            .get::<VisualCorrection<CompMixedCorrectionBundleA>>(entity)
            .map(|correction| &correction.error),
        Some(&CompMixedCorrectionBundleA(-17.0))
    );
    assert!(
        world
            .get::<VisualCorrection<CompMixedCorrectionBundleB>>(entity)
            .is_none(),
        "the uncorrected bundle member should not receive a visual correction"
    );
    assert!(
        world
            .get::<PreviousVisual<CompMixedCorrectionBundleA>>(entity)
            .is_none()
    );
    assert!(
        world
            .get::<PreviousVisual<CompMixedCorrectionBundleB>>(entity)
            .is_none()
    );
}

/// Rollback captures stale Avian velocities as decaying visual-correction
/// errors instead of snapping them while the pose glides.
#[test]
fn post_rollback_correction_smooths_velocities() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    set_correction_sampling_time(&mut stepper);
    stepper
        .client_app()
        .add_systems(FixedUpdate, replay_avian_pose);

    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 1;
    let entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            Position::default(),
            Rotation::default(),
            // Stale pre-rollback visual velocities; replay corrects them to rest.
            LinearVelocity(Vector::new(10.0, 0.0)),
            AngularVelocity(5.0),
            history(rollback_tick, Position::default()),
            history(rollback_tick, Rotation::default()),
            history(rollback_tick, LinearVelocity::default()),
            history(rollback_tick, AngularVelocity::default()),
            FrameInterpolationHistory::<Position>::default(),
            FrameInterpolationHistory::<Rotation>::default(),
            FrameInterpolationHistory::<LinearVelocity>::default(),
            FrameInterpolationHistory::<AngularVelocity>::default(),
        ))
        .id();

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);

    let world = stepper.client_app().world();
    // Live components are restored to the replayed (corrected) values.
    assert_eq!(
        world.get::<LinearVelocity>(entity),
        Some(&LinearVelocity::default())
    );
    assert_eq!(
        world.get::<AngularVelocity>(entity),
        Some(&AngularVelocity::default())
    );
    // The stale velocities are kept as decaying visual errors.
    assert_eq!(
        world
            .get::<VisualCorrection<LinearVelocity>>(entity)
            .map(|correction| &correction.error),
        Some(&LinearVelocity(Vector::new(10.0, 0.0)))
    );
    assert_eq!(
        world
            .get::<VisualCorrection<AngularVelocity>>(entity)
            .map(|correction| &correction.error),
        Some(&AngularVelocity(5.0))
    );
    assert!(
        world
            .get::<PreviousVisual<LinearVelocity>>(entity)
            .is_none()
    );
    assert!(
        world
            .get::<PreviousVisual<AngularVelocity>>(entity)
            .is_none()
    );
}
