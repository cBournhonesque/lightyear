//! Implement lightyear traits for some common bevy types
use crate::prelude::client::{InterpolationSet, PredictionSet};
use crate::shared::replication::delta::Diffable;
use crate::shared::sets::{ClientMarker, InternalReplicationSet, ServerMarker};
use avian3d::math::Scalar;
use avian3d::prelude::*;
use bevy::app::{App, FixedPostUpdate, Plugin};
use bevy::prelude::TransformSystem::TransformPropagate;
use bevy::prelude::{IntoSystemSetConfigs, PostUpdate};
use tracing::trace;

pub(crate) struct Avian3dPlugin;
impl Plugin for Avian3dPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            FixedPostUpdate,
            // Ensure PreSpawned hash set before physics runs, to avoid any physics interaction affecting it
            // TODO: maybe use observers so that we don't have any ordering requirements?
            (
                InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash,
                InternalReplicationSet::<ServerMarker>::SetPreSpawnedHash,
            )
                .before(PhysicsSet::Prepare), // Runs right before physics.
        );
        // NB: the three main physics sets in FixedPostUpdate run in this order:
        // pub enum PhysicsSet {
        //     Prepare,
        //     StepSimulation,
        //     Sync,
        // }
        app.configure_sets(
            FixedPostUpdate,
            // run physics before updating the prediction history
            (
                PredictionSet::UpdateHistory,
                PredictionSet::IncrementRollbackTick,
                InterpolationSet::UpdateVisualInterpolationState,
            )
                .after(PhysicsSet::Sync),
        );
        // if the sync happens in PostUpdate (because the visual interpolated Position/Rotation are updated every frame in
        // PostUpdate), and we want to sync them every frame because some entities (text, etc.) might depend on Transform
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
