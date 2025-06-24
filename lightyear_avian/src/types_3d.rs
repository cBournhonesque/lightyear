//! Implement lightyear traits for some common bevy types
use avian3d::math::Scalar;
use avian3d::prelude::*;
use tracing::trace;

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

    // impl Diffable for Position {
    //     type Delta = Self;
    //
    //     fn base_value() -> Self {
    //         Position::default()
    //     }
    //
    //     fn diff(&self, new: &Self) -> Self::Delta {
    //         Position(new.0 - self.0)
    //     }
    //
    //     fn apply_diff(&mut self, delta: &Self::Delta) {
    //         self.0 += delta.0;
    //     }
    // }
}

pub mod rotation {
    use super::*;
    /// We want to smoothly interpolate between the two quaternions by default,
    /// rather than using a quicker but less correct linear interpolation.
    pub fn lerp(start: &Rotation, other: &Rotation, t: f32) -> Rotation {
        start.slerp(*other, Scalar::from(t))
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
