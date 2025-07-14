//! Implement lightyear traits for some common bevy types
use avian3d::math::Scalar;
use avian3d::prelude::*;
use tracing::trace;

#[cfg(feature = "deterministic")]
use core::hash::Hasher;

pub mod position {
    use super::*;

    pub fn lerp(start: &Position, other: &Position, t: f32) -> Position {
        let u = Scalar::from(t);
        let res = Position::new(start.0 * (1.0 - u) + other.0 * u);
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }

    #[cfg(feature = "deterministic")]
    pub fn hash(pos: &Position, hasher: &mut seahash::SeaHasher) {
        hasher.write_u32(pos.x.to_bits());
        hasher.write_u32(pos.y.to_bits());
        hasher.write_u32(pos.z.to_bits());
    }
}

pub mod rotation {
    use super::*;

    /// We want to smoothly interpolate between the two quaternions by default,
    /// rather than using a quicker but less correct linear interpolation.
    pub fn lerp(start: &Rotation, other: &Rotation, t: f32) -> Rotation {
        start.slerp(*other, Scalar::from(t))
    }

    #[cfg(feature = "deterministic")]
    pub fn hash(rot: &Rotation, hasher: &mut seahash::SeaHasher) {
        let [x, y, z, w] = rot.to_array();
        hasher.write_u32(x.to_bits());
        hasher.write_u32(y.to_bits());
        hasher.write_u32(z.to_bits());
        hasher.write_u32(w.to_bits());
    }
}

pub mod linear_velocity {
    use super::*;

    pub fn lerp(start: &LinearVelocity, other: &LinearVelocity, t: f32) -> LinearVelocity {
        let u = Scalar::from(t);
        let res = LinearVelocity(start.0 * (1.0 - u) + other.0 * u);
        trace!(
            "linear velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}

pub mod angular_velocity {
    use super::*;

    pub fn lerp(start: &AngularVelocity, other: &AngularVelocity, t: f32) -> AngularVelocity {
        let u = Scalar::from(t);
        let res = AngularVelocity(start.0 * (1.0 - u) + other.0 * u);
        trace!(
            "angular velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}
