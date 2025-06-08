//! # Lightyear Avian Integration
//!
//! This crate provides integration between Lightyear and the Avian physics engine.
//!
//! It currently includes utilities for lag compensation.
#[cfg(all(not(feature = "2d"), not(feature = "3d")))]
compile_error!("either feature \"2d\" or \"3d\" must be enabled");
 
#[cfg(all(feature = "2d", feature = "3d"))]
compile_error!("feature \"2d\" and feature \"3d\" cannot be enabled at the same time");

use bevy::prelude::TransformSystem::TransformPropagate;
use bevy::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationSet;
use lightyear_prediction::plugin::PredictionSet;

/// Provides systems and components for lag compensation with Avian.
#[cfg(feature = "lag_compensation")]
pub mod lag_compensation;

#[cfg(feature = "2d")]
pub mod types_2d;
#[cfg(feature = "2d")]
pub use types_2d as types;

#[cfg(any(feature = "2d", feature = "3d"))]
mod sync;
#[cfg(feature = "3d")]
pub mod types_3d;

#[cfg(feature = "3d")]
pub use types_3d as types;

#[cfg(all(feature = "2d", not(feature = "3d")))]
use avian2d::{
    prelude::*,
    sync::{SyncConfig, SyncSet},
};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use avian3d::{
    prelude::*,
    sync::{SyncConfig, SyncSet},
};
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_interpolation::InterpolationMode;
use lightyear_replication::prelude::TransformLinearInterpolation;

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

#[cfg(any(feature = "2d", feature = "3d"))]
impl Plugin for LightyearAvianPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: the three main physics sets in FixedPostUpdate run in this order:
        // pub enum PhysicsSet {
        //     Prepare,
        //     StepSimulation,
        //     Sync,
        // }

        // TODO: this is only valid if we predict Position/Rotation!
        app.configure_sets(
            FixedPostUpdate,
            (
                // update physics
                PhysicsSet::StepSimulation,
                // TODO: is this necessary?
                // run physics before spawning the prediction history for prespawned entities that are spawned in FixedUpdate
                // we want all avian-added components (Rotation, etc.) to be inserted before we try
                // to spawn the history, so that the history is spawned at the correct time for all components
                PredictionSet::Sync,
                // save the new Position/Rotation values in the history
                PredictionSet::UpdateHistory,
                // update the Position/Rotation component value with visual correction
                PredictionSet::VisualCorrection,
                // sync any Position correction to Transform
                PhysicsSet::Sync,
                // save the Transform values for visual interpolation
                FrameInterpolationSet::Update,
            )
                .chain(),
        );

        // // TODO: this is only valid if we replicate Transform instead of Position/Rotation!
        // app.configure_sets(
        //     FixedPostUpdate,
        //     (
        //         // update physics
        //         PhysicsSet::StepSimulation,
        //         // TODO: is this necessary?
        //         // run physics before spawning the prediction history for prespawned entities that are spawned in FixedUpdate
        //         // we want all avian-added components (Rotation, etc.) to be inserted before we try
        //         // to spawn the history, so that the history is spawned at the correct time for all components
        //         PredictionSet::Sync,
        //         // sync any Position correction to Transform
        //         PhysicsSet::Sync,
        //         // save the new Transform values in the history
        //         PredictionSet::UpdateHistory,
        //         // update the Transform value with visual correction
        //         PredictionSet::VisualCorrection,
        //         // save the values for visual interpolation
        //         FrameInterpolationSet::Update,
        //     )
        //         .chain(),
        // );
        app.configure_sets(
            RunFixedMainLoop,
            PhysicsSet::Sync.in_set(RunFixedMainLoopSystem::AfterFixedMainLoop),
        );
        // TODO: this only works if Position/Rotation are replicated and Transform is FrameInterpolated!
        // Sync Pos/Rotation to Transform before applying frame interpolation to Transfrom
        app.configure_sets(
            PostUpdate,
            (
                PhysicsSet::Sync,
                FrameInterpolationSet::Interpolate,
                TransformPropagate,
            )
                .chain(),
        );

        // Sync Position/Rotation to Transform even for non RigidBody entities
        app.insert_resource(SyncConfig {
            transform_to_position: false,
            position_to_transform: true,
            ..default()
        });
        app.add_systems(
            FixedPostUpdate,
            sync::position_to_transform.in_set(SyncSet::PositionToTransform),
        );
        app.add_systems(
            PostUpdate,
            sync::position_to_transform.in_set(SyncSet::PositionToTransform),
        );
        app.configure_sets(
            PostUpdate,
            SyncSet::PositionToTransform.in_set(PhysicsSet::Sync),
        );

        // do not replicate Transform but make sure to register an interpolation function
        // for it so that we can do visual interpolation
        // (another option would be to replicate transform and not use Position/Rotation at all)
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .set_interpolation::<Transform>(TransformLinearInterpolation::lerp);
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .set_interpolation_mode::<Transform>(InterpolationMode::None);

        // Add rollback for some non-replicated resources
        // app.add_resource_rollback::<Collisions>();
        // app.add_rollback::<CollidingEntities>();
    }
}
