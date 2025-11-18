/// We want to support replicating/predicting Position/Rotation but applying FrameInterpolation on Transform.
///
/// The benefits are:
/// - Position/Rotation are smaller, which is better for prediction/serialization
/// - Correction/FrameInterpolation are a visual concern, so it would be better for them to be applied to Transform.
///
/// At the end of a rollback, we will convert Position/Rotation to Transform so that we can do FrameInterpolation and Correction in Transform space.
use avian2d::math::{AsF32, Quaternion};
use avian2d::prelude::*;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_math::curve::{EaseFunction, EasingCurve};
use bevy_math::{Curve, Isometry2d, Vec3};
use bevy_time::{Fixed, Time, Virtual};
use bevy_transform::prelude::Transform;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_frame_interpolation::{FrameInterpolate, SkipFrameInterpolation};
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_prediction::correction::{PreviousVisual, VisualCorrection};
use lightyear_prediction::manager::PredictionManager;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::prelude::PredictionRegistry;
use lightyear_replication::delta::Diffable;
#[allow(unused_imports)]
use tracing::{info, trace};

/// We want to support replicating/predicting Position/Rotation but applying FrameInterpolation on Transform.
/// The benefits are:
/// - Position/Rotation are smaller, which is better for prediction/serialization
/// - Correction/FrameInterpolation are a visual concern, so it would be better for them to be applied to Transform.
///
/// Right after RollbackEnds, we need to convert from Position/Rotation into:
/// - an update of FrameInterpolate Transform
/// - adding a VisualCorrection that can be applied to Transform
pub(crate) fn update_frame_interpolation_post_rollback(
    time: Res<Time<Fixed>>,
    timeline: Single<&LocalTimeline, With<PredictionManager>>,
    registry: Res<InterpolationRegistry>,
    mut query: Query<(
        Entity,
        &Position,
        &PreviousVisual<Position>,
        &PredictionHistory<Position>,
        &Rotation,
        &PreviousVisual<Rotation>,
        &PredictionHistory<Rotation>,
        &mut FrameInterpolate<Transform>,
        Option<&SkipFrameInterpolation>,
    )>,
    mut commands: Commands,
) {
    // NOTE: this is the overstep from the previous frame since we are running this before RunFixedMainLoop
    let overstep = time.overstep_fraction();
    let tick = timeline.tick();
    for (
        entity,
        position,
        previous_visual_position,
        position_history,
        rotation,
        previous_visual_rotation,
        rotation_history,
        mut interpolate,
        skip,
    ) in query.iter_mut()
    {
        if skip.is_some() {
            let current_transform = to_transform(position, rotation);

            interpolate.current_value = Some(current_transform);
            interpolate.previous_value = Some(current_transform);

            commands.entity(entity).remove::<(
                PreviousVisual<Position>,
                PreviousVisual<Rotation>,
                SkipFrameInterpolation,
            )>();
            continue;
        }
        // - the previous visual value is PreviousVisual
        // - the new corrected visual value that we would have displayed with the Rollback
        // is the interpolation between the last 2 states of the PredictionHistory
        // -> We want the error between the two. + we also override the FrameInterpolate with the new correct values post-rollback
        let last_correct_transform = to_transform(position, rotation);
        let (Some(before_last_pos), Some(before_last_rot)) = (
            position_history.second_most_recent(tick),
            rotation_history.second_most_recent(tick),
        ) else {
            continue;
        };
        let before_last_correct_transform = to_transform(before_last_pos, before_last_rot);
        // TODO: here we might need the Parent to correctly get the Transform! What we have is actually the GlobalTransform!
        interpolate.current_value = Some(last_correct_transform);
        interpolate.previous_value = Some(before_last_correct_transform);
        let current_visual = registry.interpolate(
            before_last_correct_transform,
            last_correct_transform,
            overstep,
        );
        // error = previous_visual - current_visual

        let previous_visual =
            to_transform(&previous_visual_position.0, &previous_visual_rotation.0);
        let error = current_visual.diff(&previous_visual);
        trace!(
            ?tick,
            ?entity,
            ?current_visual,
            ?previous_visual,
            ?error,
            // two_previous_values = ?interpolate,
            // ?history,
            "Updating VisualCorrection from Position/Rotation to Transform post rollback",
        );
        commands
            .entity(entity)
            .insert(VisualCorrection::<Isometry2d> { error })
            .remove::<(PreviousVisual<Position>, PreviousVisual<Rotation>)>();
    }
}

/// Add the visual correction error to the visual component, and
/// decay the visual correction error over time.
///
/// If it gets small enough, we remove the `VisualCorrection<C>` component.
///
/// The delta D must have a interpolation function registered in the [`InterpolationRegistry`].
pub(crate) fn add_visual_correction(
    time: Res<Time<Virtual>>,
    prediction: Res<PredictionRegistry>,
    manager: Single<&PredictionManager>,
    mut query: Query<(Entity, &mut Transform, &mut VisualCorrection<Isometry2d>)>,
    mut commands: Commands,
) {
    let r = manager.correction_policy.lerp_ratio(time.delta());
    query
        .iter_mut()
        .for_each(|(entity, mut component, mut visual_correction)| {
            if !prediction.should_rollback(
                &Position::default(),
                &Position(visual_correction.error.translation),
            ) && !prediction.should_rollback(
                &Rotation::default(),
                &Rotation::from(visual_correction.error.rotation),
            ) {
                trace!(
                    ?visual_correction,
                    "Removing VisualCorrection<Isometry2d> since it is already small enough",
                );
                commands
                    .entity(entity)
                    .remove::<VisualCorrection<Isometry2d>>();
                return;
            }
            let previous_error = visual_correction.error;
            let new_error =
                EasingCurve::new(Isometry2d::default(), previous_error, EaseFunction::Linear)
                    .sample_unchecked(r);
            component.bypass_change_detection().apply_diff(&new_error);
            trace!(
                ?entity,
                ?component,
                ?previous_error,
                ?new_error,
                ?r,
                "Applied VisualCorrection<Isometry2d> and decaying error",
            );
            visual_correction.error = new_error;
        });
}

fn to_transform(pos: &Position, rot: &Rotation) -> Transform {
    Transform {
        translation: pos.f32().extend(0.0),
        rotation: Quaternion::from(*rot).f32(),
        // TODO: handle scale ?
        scale: Vec3::ONE,
    }
}
