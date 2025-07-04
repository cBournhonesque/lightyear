#![allow(unreachable_code)]
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
use bevy_app::{
    App, FixedPostUpdate, Plugin, PostUpdate, PreUpdate, RunFixedMainLoop, RunFixedMainLoopSystem,
};
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_transform::{TransformSystem, components::Transform};
use bevy_utils::default;

use crate::sync;
use lightyear_frame_interpolation::FrameInterpolationSet;
use lightyear_interpolation::InterpolationMode;
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_prediction::plugin::PredictionSet;
use lightyear_replication::prelude::{ReplicationSet, TransformLinearInterpolation};

/// Indicate which components you are replicating over the network
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AvianReplicationMode {
    /// Replicate the Position and Rotation components.
    ///
    /// In this mode, we only replicate position/rotation and sync them to the Transform component.
    /// The Transform component is used for frame-interpolation.
    #[default]
    Position,
    /// Replicate the Transform component, but internally store the Position in the prediction history
    TransformButRollbackPosition,
    /// Replicate the Transform component, and internally store the Transform in the prediction history.
    Transform,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LightyearAvianPlugin {
    /// The replication mode to use for the Avian plugin.
    pub replication_mode: AvianReplicationMode,
    /// If True, lightyear will update the way avian syncs (Position/Rotation <> Transform) are handled.
    ///
    /// Disable if you are an advanced user and want to handle the syncs manually.
    pub update_syncs_manually: bool,
}

impl Plugin for LightyearAvianPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: the three main physics sets in FixedPostUpdate run in this order:
        // pub enum PhysicsSet {
        //     Prepare,
        //     StepSimulation,
        //     Sync,
        // }

        // just in case the user is running physics in RunFixedMainLoop...
        app.configure_sets(
            RunFixedMainLoop,
            PhysicsSet::Sync.in_set(RunFixedMainLoopSystem::AfterFixedMainLoop),
        );

        match self.replication_mode {
            AvianReplicationMode::Position => {
                app.configure_sets(
                    FixedPostUpdate,
                    (
                        // update physics
                        PhysicsSet::StepSimulation,
                        // run physics before spawning we sync so that PreSpawned entities have accurate Position/Rotation values in their history
                        PredictionSet::Sync,
                        // the visual correction are what we actually want to display, so they must happen before syncing from Position to Transform
                        PredictionSet::VisualCorrection,
                        PhysicsSet::Sync,
                        // the transform value has to be updated before we can store it for FrameInterpolation
                        FrameInterpolationSet::Update,
                    )
                        .chain(),
                );
                // In case the user is running physics in PostUpdate: Sync Pos/Rotation to Transform before applying frame interpolation to Transform
                app.configure_sets(
                    PostUpdate,
                    (
                        PhysicsSet::Sync,
                        FrameInterpolationSet::Interpolate,
                        TransformSystem::TransformPropagate,
                    )
                        .chain(),
                );

                if !self.update_syncs_manually {
                    // Sync Position/Rotation to Transform even for non RigidBody entities
                    app.insert_resource(SyncConfig {
                        // there is no need to sync Transform to Position since we are not replicating Transform. It might not hurt to enable this? This is mostly disabled as an optimization.
                        transform_to_position: false,
                        // Disable the transform to position sync because we are doing it manually with our custom position_to_transform systems
                        position_to_transform: false,
                        ..default()
                    });
                    // We manually sync Position/Rotation to Transform. We do it even
                    // for entities that are not RigidBodies (for example Interpolated entities)
                    app.add_systems(
                        FixedPostUpdate,
                        sync::position_to_transform.in_set(SyncSet::PositionToTransform),
                    );
                    app.add_systems(
                        PostUpdate,
                        sync::position_to_transform.in_set(SyncSet::PositionToTransform),
                    );
                }
            }
            AvianReplicationMode::TransformButRollbackPosition => {
                unimplemented!();
                // The main issue with this mode is that the Transform component gets replicated, but avian internally works on Position and Rotation components. So we need
                // to ensure that in PreUpdate, after receiving the Transform component, we sync it to Position and Rotation.
                // PreUpdate:
                // - we need an initial Sync from Transform to Position in PreUpdate
                // - we need to still do the rollback check on Position so the Position should be a non-networked component with PredictionMode = Full?
                // FixedPostUpdate:
                // - we need a sync from Position to Transform in FixedPostUpdate
                // Avian doesn't support updating the sync config separately for two schedules.

                app.configure_sets(
                    PreUpdate,
                    (
                        ReplicationSet::Receive,
                        // sync Transform to Position
                        PhysicsSet::Sync,
                        PredictionSet::Sync,
                    )
                        .chain(),
                );

                // the FixedPostUpdate ordering is similar to the ReplicatePosition mode
                app.configure_sets(
                    FixedPostUpdate,
                    (
                        // update physics
                        PhysicsSet::StepSimulation,
                        // run physics before spawning we sync so that PreSpawned entities have accurate Position/Rotation values in their history
                        PredictionSet::Sync,
                        // the visual correction are what we actually want to display, so they must happen before syncing from Position to Transform
                        PredictionSet::VisualCorrection,
                        PhysicsSet::Sync,
                        // the transform value has to be updated before we can store it for FrameInterpolation
                        FrameInterpolationSet::Update,
                    )
                        .chain(),
                );

                // TODO: handle syncs
            }
            AvianReplicationMode::Transform => {
                // TODO: the rollback check is done with Transform (so no need to sync Transform to Position in PreUpdate)
                //  however we still need a Transform->Position sync before running the StepSimulation!
                //  (and a Position->Transform sync in FixedPostUpdate::PhysicsSet::Sync)
                //  so we need to split the sync logic into PreUpdate and FixedPostUpdate
                unimplemented!();
                app.configure_sets(
                    FixedPostUpdate,
                    (
                        // update physics
                        PhysicsSet::StepSimulation,
                        // run physics before spawning we sync so that PreSpawned entities have accurate Position/Rotation values in their history
                        PredictionSet::Sync,
                        // sync any Corrected Position to Transform
                        PhysicsSet::Sync,
                        // save the new Corrected Transform values in the history
                        PredictionSet::UpdateHistory,
                        // update the Transform value with visual correction
                        PredictionSet::VisualCorrection,
                        // save the values for visual interpolation
                        FrameInterpolationSet::Update,
                    )
                        .chain(),
                );
            }
        }

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
