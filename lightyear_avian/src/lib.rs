//! # Lightyear Avian Integration
//!
//! This crate provides integration between Lightyear and the Avian physics engine.
//!
//! It currently includes utilities for lag compensation.

use bevy::prelude::TransformSystem::TransformPropagate;
use bevy::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationSet;
use lightyear_prediction::plugin::PredictionSet;

/// Provides systems and components for lag compensation with Avian.
#[cfg(feature = "lag_compensation")]
pub mod lag_compensation;

#[cfg(feature = "2d")]
pub mod avian2d;
#[cfg(all(feature = "2d", not(feature = "3d")))]
use ::avian2d::prelude::PhysicsSet;

#[cfg(feature = "3d")]
pub mod avian3d;
#[cfg(all(feature = "3d", not(feature = "2d")))]
use ::avian3d::prelude::PhysicsSet;

/// Commonly used items for Lightyear Avian integration.
pub mod prelude {
    #[cfg(feature = "lag_compensation")]
    pub use crate::lag_compensation::{
        history::{
            AabbEnvelopeHolder, LagCompensationConfig, LagCompensationHistory,
            LagCompensationPlugin, LagCompensationSet,
        },
        query::LagCompensationSpatialQuery,
    };
}


pub struct LightyearAvianPlugin;

impl Plugin for LightyearAvianPlugin {
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
                FrameInterpolationSet::Update,
            )
                .chain(),
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
                FrameInterpolationSet::Interpolate,
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