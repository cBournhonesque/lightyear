//! Handle Correction for avian. We need to handle this manually because we replicate 
//! Position and Rotation, but the visual representation is a Transform.
//! 
//! The flow is:
//! - PreUpdate:
//!   - rollback check. We store the previous Position/Rotation in PreviousVisual<Position>/PreviousVisual<Rotation>
//!   - apply rollback
//!   - end rollback
//!     - set current/previous values to be the last 2 values from the history
//!     - compute the visual using the previous frame's overstep
//!     - compute the error between the new visual and the previous visual (when they both use the same overstep)
//! - BeforeFixedUpdate:
//!   - restore Position/Rotation to the tick value from FrameInterpolation
//! - FixedUpdate:
//!   - Run Physics simulation
//!   - Sync Position/Rotation to Transform
//!   - Update FrameInterpolation<Transform> with the new Transform value
//! - PostUpdate:
//!   - interpolate with FrameInterpolation<Transform>
//!   - apply VisualCorrection<Transform> if present
//!   - apply TransformPropagation
//! 
//! If the user is running FrameInterpolation<Transform>, we need to either:
//! - in end_rollback, compute the error in Position/Rotation space, then convert it to Transform space? How do we handle the Local/Global transform?
//! - or in end_rollback, compute the error in Position/Rotation space.
//! 
//! Id the user is running FrameInterpolation<GlobalTransform>, we can:
//! - 

use bevy_app::{App, Plugin};
use bevy_ecs::change_detection::Res;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::{Commands, Query, Single, With};
use tracing::trace;
use lightyear_core::prelude::LocalTimeline;
use lightyear_frame_interpolation::FrameInterpolate;
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_prediction::correction::VisualCorrection;
use lightyear_prediction::manager::PredictionManager;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::SyncComponent;
use lightyear_replication::delta::Diffable;

pub struct Correction2DPlugin;


impl Plugin for Correction2DPlugin {
    fn build(&self, app: &mut App) {
        todo!()
    }
}


/// After the rollback is over, we need to update the values in the [`FrameInterpolate<C>`] component.
///
/// If we have correction enabled, then we can compute the error between the previous visual value
/// [`PreviousVisual<C>`] and the new visual value.
pub(crate) fn update_frame_interpolation_post_rollback(
    time: Res<Time<Fixed>>,
    // only run if there is a VisualCorrection<C> to do.
    timeline: Single<&LocalTimeline, With<PredictionManager>>,
    registry: Res<InterpolationRegistry>,
    mut query: Query<(
        Entity,
        &mut Position,
        &mut Rotation,
        &PreviousVisual<Position>,
        &PreviousVisual<Rotation>,
        &PredictionHistory<Position>,
        &PredictionHistory<Rotation>,
        &mut FrameInterpolate<Transform>,
    )>,
    mut commands: Commands,
) {
    // NOTE: this is the overstep from the previous frame since we are running this before RunFixedMainLoop
    let overstep = time.overstep_fraction();
    let tick = timeline.tick();
    for (entity, position, rotation, previous_visual_position, previous_visual_rotation, position_history, rotation_history, interpolate) in query.iter_mut() {
        k
        interpolate.current_value = Some(component.clone());
        interpolate.previous_value = history.nth_most_recent(1).cloned();
        let Some(previous) = &interpolate.previous_value else {
            continue;
        };
        let current_visual = registry.interpolate(previous.clone(), component.clone(), overstep);
        // error = previous_visual - current_visual
        let error = current_visual.diff(&previous_visual.0);
        trace!(
            ?tick,
            ?entity,
            ?current_visual,
            ?previous_visual,
            ?error,
            "Updating VisualCorrection post rollback for {:?}",
            core::any::type_name::<C>()
        );
        commands
            .entity(entity)
            .insert(VisualCorrection::<C> { error })
            .remove::<PreviousVisual<C>>();
    }
}