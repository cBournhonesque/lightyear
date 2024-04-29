//! Implement lightyear traits for some common bevy types
use std::ops::{Add, Mul};

use bevy::prelude::EntityMapper;
use bevy_xpbd_2d::components::*;
use tracing::trace;

// pub use angular_velocity::*;
// pub use linear_velocity::*;
// pub use position::*;
// pub use rotation::*;

use crate::client::components::{LerpFn, SyncComponent};
use crate::prelude::Message;

pub mod position {
    use super::*;
    use crate::_internal::LinearInterpolator;

    pub fn lerp(start: &Position, other: &Position, t: f32) -> Position {
        let res = Position::new(start.0 * (1.0 - t) + other.0 * t);
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

pub mod rotation {
    use super::*;
    use crate::_internal::LinearInterpolator;

    pub fn lerp(start: &Rotation, other: &Rotation, t: f32) -> Rotation {
        let shortest_angle =
            ((((other.as_degrees() - start.as_degrees()) % 360.0) + 540.0) % 360.0) - 180.0;
        let res = Rotation::from_degrees(start.as_degrees() + shortest_angle * t);
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
}

pub mod linear_velocity {
    use super::*;
    use crate::_internal::LinearInterpolator;
    pub fn lerp(start: &LinearVelocity, other: &LinearVelocity, t: f32) -> LinearVelocity {
        let res = LinearVelocity(start.0 * (1.0 - t) + other.0 * t);
        trace!(
            "linear velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start,
            other,
            t,
            res
        );
        res
    }
}

pub mod angular_velocity {
    use super::*;
    use crate::_internal::LinearInterpolator;

    pub fn lerp(start: &AngularVelocity, other: &AngularVelocity, t: f32) -> AngularVelocity {
        let res = AngularVelocity(start.0 * (1.0 - t) + other.0 * t);
        trace!(
            "angular velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start,
            other,
            t,
            res
        );
        res
    }
}
