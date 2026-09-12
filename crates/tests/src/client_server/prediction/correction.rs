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
use lightyear_prediction::prelude::{CorrectionPolicy, Predicted, VisualCorrection};
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

/// A policy that gives up nothing, so a correction can be observed exactly as it
/// was recorded instead of part-way through its decay.
fn freeze_correction(stepper: &mut ClientServerStepper, entity: Entity) {
    stepper
        .client_app()
        .world_mut()
        .entity_mut(entity)
        .insert(CorrectionPolicy::new(1.0, Duration::from_secs(3600)));
}

/// Runs the rest of the frame after `PreUpdate`: frame interpolation writes the
/// value this frame renders, the shared system records the correction from it,
/// and the apply adds it.
fn finish_frame_with_correction(stepper: &mut ClientServerStepper) {
    stepper.client_app().world_mut().run_schedule(PostUpdate);
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

/// A rollback records the jump between the value on screen and the value the
/// frame renders after the replay. The measurement is taken in `PostUpdate`,
/// from the value frame interpolation wrote, so what the correction holds is
/// exactly the pre-rollback render.
#[test]
fn post_rollback_correction_uses_the_frame_interpolated_value() {
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
    freeze_correction(&mut stepper, entity);

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);
    // The replay restored the simulated values; the frame history was repaired
    // so the frame interpolator can sample them.
    {
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
    }

    finish_frame_with_correction(&mut stepper);

    let world = stepper.client_app().world();
    // The correction is the jump from the pre-rollback render to the value the
    // frame renders, so live plus error holds the render where it was.
    let error_a = world
        .get::<VisualCorrection<CompCorrectionBundleA>>(entity)
        .map(|correction| correction.error.clone())
        .expect("the bundle rule's value should produce a correction");
    let error_b = world
        .get::<VisualCorrection<CompCorrectionBundleB>>(entity)
        .map(|correction| correction.error.clone())
        .expect("the bundle rule's value should produce a correction");
    assert_ne!(
        error_a,
        CompCorrectionBundleA(0.0),
        "a rollback must leave a correction"
    );
    assert_ne!(
        error_b,
        CompCorrectionBundleB(0.0),
        "both bundle members are corrected from the same sampled frame"
    );
    // live = interpolated + error, so this is the pre-rollback render.
    assert_eq!(
        world.get::<CompCorrectionBundleA>(entity),
        Some(&CompCorrectionBundleA(1.0)),
        "the render is held at the pre-rollback value"
    );
    assert_eq!(
        world.get::<CompCorrectionBundleB>(entity),
        Some(&CompCorrectionBundleB(2.0)),
        "the render is held at the pre-rollback value"
    );
    // The saved value was spent recording the correction.
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

/// A bundle member registered for prediction but not correction contributes its
/// sampled value to another member's correction without receiving correction
/// state of its own.
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
    freeze_correction(&mut stepper, entity);

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);
    finish_frame_with_correction(&mut stepper);

    let world = stepper.client_app().world();
    // The corrected member holds its render; the uncorrected member is left to
    // the destination timeline's own value.
    assert_eq!(
        world.get::<CompMixedCorrectionBundleA>(entity),
        Some(&CompMixedCorrectionBundleA(1.0)),
        "the corrected member's render is held at the pre-rollback value"
    );
    assert_ne!(
        world
            .get::<VisualCorrection<CompMixedCorrectionBundleA>>(entity)
            .map(|correction| correction.error.clone()),
        Some(CompMixedCorrectionBundleA(0.0)),
        "the corrected member needs a correction for the sampled jump"
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

/// Rollback captures stale Avian velocities as decaying visual-correction errors
/// instead of snapping them while the pose glides.
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
    freeze_correction(&mut stepper, entity);

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.client_app().world_mut().run_schedule(PreUpdate);
    finish_frame_with_correction(&mut stepper);

    let world = stepper.client_app().world();
    // While the errors decay, the velocities the renderer sees stay at the stale
    // values instead of snapping to the replayed ones.
    assert_eq!(
        world.get::<LinearVelocity>(entity),
        Some(&LinearVelocity(Vector::new(10.0, 0.0))),
        "the stale velocity is held, not snapped"
    );
    assert_eq!(
        world.get::<AngularVelocity>(entity),
        Some(&AngularVelocity(5.0)),
        "the stale velocity is held, not snapped"
    );
    assert_ne!(
        world
            .get::<VisualCorrection<LinearVelocity>>(entity)
            .map(|correction| correction.error.clone()),
        Some(LinearVelocity::default()),
        "a correction is carrying the difference"
    );
    assert_ne!(
        world
            .get::<VisualCorrection<AngularVelocity>>(entity)
            .map(|correction| correction.error.clone()),
        Some(AngularVelocity(0.0)),
        "a correction is carrying the difference"
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
