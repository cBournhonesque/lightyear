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
use bevy_app::prelude::*;
pub struct Correction2DPlugin;

impl Plugin for Correction2DPlugin {
    fn build(&self, _app: &mut App) {
        todo!()
    }
}
