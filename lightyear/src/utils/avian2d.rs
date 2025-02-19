//! Implement lightyear traits for some common bevy types
use crate::prelude::client::{InterpolationSet, PredictionSet};
use crate::shared::replication::delta::Diffable;
use avian2d::math::Scalar;
use avian2d::prelude::*;
use bevy::app::{RunFixedMainLoop, RunFixedMainLoopSystem};
use bevy::prelude::TransformSystem::TransformPropagate;
use bevy::prelude::{App, FixedPostUpdate, Plugin};
use bevy::prelude::{IntoSystemSetConfigs, PostUpdate};
use tracing::trace;

pub(crate) struct Avian2dPlugin;

impl Plugin for Avian2dPlugin {
    fn build(&self, app: &mut App) {
        // NB: the three main physics sets in FixedPostUpdate run in this order:
        // pub enum PhysicsSet {
        //     Prepare,
        //     StepSimulation,
        //     Sync,
        // }
        app.configure_sets(
            FixedPostUpdate,
            (
                // update physics
                PhysicsSet::StepSimulation,
                // run physics before spawning the prediction history for prespawned entities that are spawned in FixedUpdate
                // we want all avian-added components (Rotation, etc.) to be inserted before we try
                // to spawn the history, so that the history is spawned at the correct time for all components
                PredictionSet::Sync,
                // save the new values in the history
                PredictionSet::UpdateHistory,
                // update the component value with visual correction
                PredictionSet::VisualCorrection,
                // sync any Position correction to Transform
                PhysicsSet::Sync,
                // save the values for visual interpolation
                InterpolationSet::UpdateVisualInterpolationState,
            )
                .chain()
        );
        app.configure_sets(
            RunFixedMainLoop,
            PhysicsSet::Sync.in_set(RunFixedMainLoopSystem::AfterFixedMainLoop),
        );
        // if we are syncing Position/Rotation in PostUpdate (not in FixedLast because FixedLast might not run
        // in some frames), and running VisualInterpolation for Position/Rotation,
        // we want to first interpolate and then sync to transform
        app.configure_sets(
            PostUpdate,
            (
                InterpolationSet::VisualInterpolation,
                PhysicsSet::Sync,
                TransformPropagate,
            )
                .chain(),
        );

        // Add rollback for some non-replicated resources
        // app.add_resource_rollback::<Collisions>();
        // app.add_rollback::<CollidingEntities>();
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
