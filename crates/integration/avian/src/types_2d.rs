//! Implement lightyear traits for some common bevy types
use avian2d::math::Scalar;
use avian2d::prelude::*;
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
    }
}

pub mod rotation {
    use super::*;

    pub fn lerp(start: &Rotation, other: &Rotation, t: f32) -> Rotation {
        let u = Scalar::from(t);
        let shortest_angle =
            ((((other.as_degrees() - start.as_degrees()) % 360.0) + 540.0) % 360.0) - 180.0;
        let res = Rotation::degrees(start.as_degrees() + shortest_angle * u);
        // // as_radians() returns a value between -Pi and Pi
        // // add Pi to get positive values, for interpolation
        // let res = Rotation::from_radians(
        //     (start.as_radians() + std::f32::consts::PI) * (1.0 - t)
        //         + (other.as_radians() + std::f32::consts::PI) * t,
        // );
        trace!(
            "rotation lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start.as_degrees(),
            other.as_degrees(),
            t,
            res.as_degrees()
        );
        res
    }

    #[cfg(feature = "deterministic")]
    pub fn hash(rot: &Rotation, hasher: &mut seahash::SeaHasher) {
        hasher.write_u32(rot.cos.to_bits());
        hasher.write_u32(rot.sin.to_bits());
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
