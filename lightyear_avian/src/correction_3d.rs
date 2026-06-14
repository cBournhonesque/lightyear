/// We want to support replicating/predicting Position/Rotation but applying FrameInterpolation on Transform.
///
/// The benefits are:
/// - Position/Rotation are smaller, which is better for prediction/serialization
/// - Correction/FrameInterpolation are a visual concern, so it would be better for them to be applied to Transform.
///
/// At the end of a rollback, we will convert Position/Rotation to Transform so that we can do FrameInterpolation and Correction in Transform space.
use avian3d::math::AsF32;
use avian3d::prelude::*;
use bevy_ecs::prelude::*;
use bevy_math::curve::{EaseFunction, EasingCurve};
use bevy_math::{Curve, Isometry3d};
use bevy_time::{Fixed, Time, Virtual};
use bevy_transform::components::{GlobalTransform, Transform};
use lightyear_core::prelude::LocalTimeline;
use lightyear_frame_interpolation::{FrameInterpolate, SkipFrameInterpolation};
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_prediction::correction::{PreviousVisual, VisualCorrection};
use lightyear_prediction::manager::PredictionManager;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::prelude::PredictionRegistry;
use lightyear_replication::delta::Diffable;
use tracing::trace;

type CorrectionTransformComponents = (
    Entity,
    &'static Position,
    &'static PreviousVisual<Position>,
    &'static PredictionHistory<Position>,
    &'static Rotation,
    &'static PreviousVisual<Rotation>,
    &'static PredictionHistory<Rotation>,
    &'static mut FrameInterpolate<Transform>,
    Option<&'static SkipFrameInterpolation>,
    Option<&'static ChildOf>,
    &'static Transform,
);

type ParentComponents = (
    Option<&'static GlobalTransform>,
    Option<&'static Position>,
    Option<&'static Rotation>,
);

/// We want to support replicating/predicting Position/Rotation but applying FrameInterpolation on Transform.
/// The benefits are:
/// - Position/Rotation are smaller, which is better for prediction/serialization
/// - Correction/FrameInterpolation are a visual concern, so it would be better for them to be applied to Transform.
///
/// Right after RollbackEnds, we need to convert from Position/Rotation into:
/// - an update of FrameInterpolate Transform
/// - adding a VisualCorrection that can be applied to Transform
///
/// `Position` operates in global space. When an entity has a parent, convert the
/// global physics pose into the entity's local `Transform` before updating visual
/// interpolation/correction so Bevy transform propagation can compose it correctly.
pub(crate) fn update_frame_interpolation_post_rollback(
    time: Res<Time<Fixed>>,
    local_timeline: Res<LocalTimeline>,
    predicted: Single<(), With<PredictionManager>>,
    registry: Res<InterpolationRegistry>,
    mut query: Query<CorrectionTransformComponents>,
    parents: Query<ParentComponents>,
    mut commands: Commands,
) {
    // NOTE: this is the overstep from the previous frame since w are running this before RunFixedMainLoop
    let overstep = time.overstep_fraction();
    let tick = local_timeline.tick();
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
        parent,
        transform,
    ) in query.iter_mut()
    {
        if skip.is_some() {
            let current_transform = to_transform(transform, position, rotation, parent, &parents);

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
        let last_correct_transform = to_transform(transform, position, rotation, parent, &parents);
        let (Some(before_last_pos), Some(before_last_rot)) = (
            position_history.get(tick - 1),
            rotation_history.get(tick - 1),
        ) else {
            continue;
        };
        let before_last_correct_transform = to_transform(
            transform,
            before_last_pos,
            before_last_rot,
            parent,
            &parents,
        );
        interpolate.current_value = Some(last_correct_transform);
        interpolate.previous_value = Some(before_last_correct_transform);
        let current_visual = registry.interpolate(
            before_last_correct_transform,
            last_correct_transform,
            overstep,
        );
        // error = previous_visual - current_visual

        let previous_visual = to_transform(
            transform,
            &previous_visual_position.0,
            &previous_visual_rotation.0,
            parent,
            &parents,
        );
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
            .insert(VisualCorrection::<Isometry3d> { error })
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
    mut query: Query<(Entity, &mut Transform, &mut VisualCorrection<Isometry3d>)>,
    mut commands: Commands,
) {
    let r = manager.correction_policy.lerp_ratio(time.delta());
    query
        .iter_mut()
        .for_each(|(entity, mut component, mut visual_correction)| {
            if !prediction.should_rollback(
                &Position::default(),
                &Position(visual_correction.error.translation.into()),
            ) && !prediction.should_rollback(
                &Rotation::default(),
                &Rotation::from(visual_correction.error.rotation),
            ) {
                trace!(
                    ?visual_correction,
                    "Removing VisualCorrection<Isometry3d> since it is already small enough",
                );
                commands
                    .entity(entity)
                    .remove::<VisualCorrection<Isometry3d>>();
                return;
            }
            let previous_error = visual_correction.error;
            let new_error =
                EasingCurve::new(Isometry3d::default(), previous_error, EaseFunction::Linear)
                    .sample_unchecked(r);
            component.bypass_change_detection().apply_diff(&new_error);
            trace!(
                ?entity,
                ?component,
                ?previous_error,
                ?new_error,
                ?r,
                "Applied VisualCorrection<Isometry3d> and decaying error",
            );
            visual_correction.error = new_error;
        });
}

fn to_transform(
    transform: &Transform,
    pos: &Position,
    rot: &Rotation,
    parent: Option<&ChildOf>,
    parents: &Query<ParentComponents>,
) -> Transform {
    let mut transform = *transform;
    if let Some(&ChildOf(parent)) = parent
        && let Ok((parent_global_transform, parent_pos, parent_rot)) = parents.get(parent)
    {
        let parent_transform = parent_global_transform
            .unwrap_or(&GlobalTransform::IDENTITY)
            .compute_transform();
        let parent_pos = parent_pos.map_or(parent_transform.translation, |pos| pos.f32());
        let parent_rot = parent_rot.map_or(parent_transform.rotation, |rot| rot.f32());
        let parent_scale = parent_transform.scale;
        let parent_transform = Transform::from_translation(parent_pos)
            .with_rotation(parent_rot)
            .with_scale(parent_scale);

        let new_transform =
            GlobalTransform::from(Transform::from_translation(pos.f32()).with_rotation(rot.f32()))
                .reparented_to(&GlobalTransform::from(parent_transform));

        transform.translation = new_transform.translation;
        transform.rotation = new_transform.rotation;
    } else {
        transform.translation = pos.f32();
        transform.rotation = rot.f32();
    }

    transform
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::system::RunSystemOnce;

    #[derive(Resource, Default)]
    struct OutputTransform(Option<Transform>);

    #[test]
    fn child_global_pose_converts_to_local_transform() {
        fn system(
            mut output: ResMut<OutputTransform>,
            query: Single<(&Transform, &Position, &Rotation, &ChildOf)>,
            parents: Query<ParentComponents>,
        ) {
            let (transform, position, rotation, child_of) = *query;
            output.0 = Some(to_transform(
                transform,
                position,
                rotation,
                Some(child_of),
                &parents,
            ));
        }

        let mut world = World::new();
        world.init_resource::<OutputTransform>();
        let parent = world
            .spawn((
                GlobalTransform::from(Transform::from_xyz(1.0, 1.0, 1.0)),
                Position::new(bevy_math::Vec3::new(1.0, 1.0, 1.0)),
                Rotation::default(),
            ))
            .id();
        world.spawn((
            ChildOf(parent),
            Transform::from_scale(bevy_math::Vec3::splat(2.0)),
            Position::new(bevy_math::Vec3::new(3.0, 4.0, 5.0)),
            Rotation::default(),
        ));

        world.run_system_once(system).unwrap();

        let child_local_transform = world.resource::<OutputTransform>().0.unwrap();
        assert_eq!(child_local_transform.scale, bevy_math::Vec3::splat(2.0));
        assert!((child_local_transform.translation.x - 2.0).abs() < 0.0001);
        assert!((child_local_transform.translation.y - 3.0).abs() < 0.0001);
        assert!((child_local_transform.translation.z - 4.0).abs() < 0.0001);
    }
}
