//! Implement lightyear traits for some common bevy types

use bevy::prelude::{Quat, Transform};
use tracing::trace;

use crate::client::components::LerpFn;

pub struct TransformLinearInterpolation;

impl LerpFn<Transform> for TransformLinearInterpolation {
    fn lerp(start: &Transform, other: &Transform, t: f32) -> Transform {
        let translation = start.translation * (1.0 - t) + other.translation * t;
        let rotation = start.rotation.slerp(other.rotation, t);
        let scale = start.scale * (1.0 - t) + other.scale * t;
        let res = Transform {
            translation,
            rotation,
            scale,
        };
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start,
            other,
            t,
            res
        );
        res
    }
}

/// Perform a spherical linear interpolation between two quaternions
pub struct QuatSphericalLinearInterpolation;

impl LerpFn<Quat> for QuatSphericalLinearInterpolation {
    fn lerp(start: &Quat, other: &Quat, t: f32) -> Quat {
        start.slerp(*other, t)
    }
}
