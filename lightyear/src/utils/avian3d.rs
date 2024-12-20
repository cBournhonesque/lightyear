//! Implement lightyear traits for some common bevy types
use crate::prelude::client::{InterpolationSet, PredictionSet};
use crate::shared::replication::delta::Diffable;
use crate::shared::sets::{ClientMarker, InternalReplicationSet, ServerMarker};
use avian3d::math::Scalar;
use avian3d::prelude::*;
use bevy::app::{App, FixedPostUpdate, Plugin};
use bevy::math::Quat;
use bevy::prelude::IntoSystemSetConfigs;
use tracing::trace;

pub(crate) struct Avian3dPlugin;
impl Plugin for Avian3dPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            FixedPostUpdate,
            (
                // run physics after setting the PreSpawned hash to avoid any physics interaction affecting the hash
                // TODO: maybe use observers so that we don't have any ordering requirements?
                (
                    InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash,
                    InternalReplicationSet::<ServerMarker>::SetPreSpawnedHash,
                ),
                (
                    PhysicsSet::Prepare,
                    PhysicsSet::StepSimulation,
                    PhysicsSet::Sync,
                ),
                // run physics before updating the prediction history
                (
                    PredictionSet::UpdateHistory,
                    PredictionSet::IncrementRollbackTick,
                    InterpolationSet::UpdateVisualInterpolationState,
                ),
            )
                .chain(),
        );
    }
}

pub mod position {
    use super::*;

    pub fn lerp(start: &Position, other: &Position, t: f32) -> Position {
        let u = Scalar::from(t);
        let res = Position::new(start.0 * (1.0 - u) + other.0 * u);
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start,
            other,
            t,
            res
        );
        res
    }

    impl Diffable for Position {
        type Delta = Self;

        fn base_value() -> Self {
            Position::default()
        }

        fn diff(&self, new: &Self) -> Self::Delta {
            Position(new.0 - self.0)
        }

        fn apply_diff(&mut self, delta: &Self::Delta) {
            self.0 += delta.0;
        }
    }
}

/// copied from Animatable trait in bevy_animation, which we don't want as a dep because
/// it pulls in all the render stuff, and we might need to interpolate on a headless server.
fn interpolate_quat(a: &Quat, b: &Quat, t: f32) -> Quat {
    // We want to smoothly interpolate between the two quaternions by default,
    // rather than using a quicker but less correct linear interpolation.
    a.slerp(*b, t)
}

pub mod rotation {
    use super::*;

    pub fn lerp(start: &Rotation, other: &Rotation, t: f32) -> Rotation {
        Rotation(interpolate_quat(&start.0, &other.0, t))
    }
}

pub mod linear_velocity {
    use super::*;

    pub fn lerp(start: &LinearVelocity, other: &LinearVelocity, t: f32) -> LinearVelocity {
        let u = Scalar::from(t);
        let res = LinearVelocity(start.0 * (1.0 - u) + other.0 * u);
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

    pub fn lerp(start: &AngularVelocity, other: &AngularVelocity, t: f32) -> AngularVelocity {
        let u = Scalar::from(t);
        let res = AngularVelocity(start.0 * (1.0 - u) + other.0 * u);
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
