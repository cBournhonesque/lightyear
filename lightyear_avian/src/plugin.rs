#![allow(unreachable_code)]
#[cfg(all(feature = "2d", not(feature = "3d")))]
use avian2d::{
    prelude::*,
    physics_transform::*,
    sync::{SyncConfig, SyncSet},
};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use avian3d::{
    prelude::*,
    physics_transform::*,
    sync::{SyncConfig, SyncSet},
};
use bevy_app::{
    App, FixedPostUpdate, Plugin, PostUpdate, PreUpdate, RunFixedMainLoop, RunFixedMainLoopSystems,
};
use bevy_ecs::change_detection::Res;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel};
use bevy_transform::{TransformSystems, components::Transform};
use bevy_transform::systems::{mark_dirty_trees, propagate_parent_transforms, sync_simple_transforms};
use bevy_utils::default;

use crate::sync;
use lightyear_frame_interpolation::FrameInterpolationSet;
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_prediction::plugin::PredictionSet;
use lightyear_prediction::prelude::{PredictionAppRegistrationExt, RollbackSet};
use lightyear_replication::prelude::{ReplicationSet, TransformLinearInterpolation};

/// Indicate which components you are replicating over the network
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AvianReplicationMode {
    /// Replicate the Position component.
    /// PredictionHistory, Correction and FrameInterpolation also apply to Position.
    #[default]
    Position,
    /// Replicate the Position component.
    /// PredictionHistory, Correction and FrameInterpolation apply on Transform
    PositionButTransformCorrection,
    /// Replicate the Transform component.
    /// PredictionHistory, Correction and FrameInterpolation also apply to Transform.
    Transform,
}

/// Plugin that integrates Avian with Lightyear for networked physics replication.
///
/// NOTE: this plugin is NOT added automatically by ClientPlugins/ServerPlugins, you have to add it manually!
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LightyearAvianPlugin {
    /// The replication mode to use for the Avian plugin.
    pub replication_mode: AvianReplicationMode,
    /// If True, lightyear will update the way avian syncs (Position/Rotation <> Transform) are handled.
    ///
    /// Disable if you are an advanced user and want to handle the syncs manually.
    pub update_syncs_manually: bool,
    /// If True, the plugin will rollback resources that are not replicated, such as Collisions.
    /// Enable this if you are using deterministic replication (i.e. are not replicating state)
    pub rollback_resources: bool,
}

impl Plugin for LightyearAvianPlugin {
    fn build(&self, app: &mut App) {
        match self.replication_mode {
            AvianReplicationMode::Position => {

                app.configure_sets(
                    FixedPostUpdate,
                    (
                        // update physics before we store the new Position in the history
                        (PhysicsSystems::StepSimulation, PredictionSet::UpdateHistory).chain(),
                        // If using FrameInterpolation<Transform>, the Transform value has to be updated
                        // before we can store it for FrameInterpolation.
                        // If using FrameInterpolation<Position>, make sure that the FrameInterpolation value
                        // use the new physics Position.
                        (PhysicsSystems::Writeback, FrameInterpolationSet::Update).chain(),
                    )
                );
                app.configure_sets(
                    PostUpdate,
                    (
                        FrameInterpolationSet::Interpolate,
                        // We don't want the correction to affect the FrameInterpolation values
                        RollbackSet::VisualCorrection,
                        // In case the user is running FrameInterpolation or Correction for Position/Rotation,
                        // we need to sync the result from FrameInterpolation/Correction to Transform
                        PhysicsSystems::Writeback,
                        TransformSystems::Propagate,
                    )
                        .chain(),
                );
                // we need to include PhysicsTransformPlugin in PostUpdate as well so that
                // Position -> Transform runs after Correction has been applied on the Position component
                app.add_plugins(PhysicsTransformPlugin::new(PostUpdate));
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
                        PredictionSet::UpdateHistory,
                        PhysicsSet::Sync,
                        // the transform value has to be updated (from Position) before we can store it for FrameInterpolation
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
                app.configure_sets(
                    FixedPostUpdate,
                    (
                        // TransformToPosition
                        PhysicsSystems::Prepare,
                        // update physics
                        PhysicsSystems::StepSimulation,
                        // sync any Corrected Position to Transform
                        PhysicsSet::Sync,
                        // save the new Corrected Transform values in the history
                        PredictionSet::UpdateHistory,
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

        if self.rollback_resources {
            app.init_resource::<ContactGraph>();
            // Add rollback for some non-replicated resources
            app.add_resource_rollback::<ContactGraph>();
            app.add_resource_rollback::<ConstraintGraph>();
            app.add_rollback::<CollidingEntities>();
        }
    }
}


impl LightyearAvianPlugin {
    fn sync_transform_to_position(app: &mut App, schedule: impl ScheduleLabel) {
        app.init_resource::<PhysicsTransformConfig>();
        let schedule = schedule.intern();
        // Manually propagate Transform to GlobalTransform before running physics
        app.configure_sets(
            schedule,
            (
                PhysicsTransformSystems::Propagate,
                PhysicsTransformSystems::TransformToPosition,
            )
                .chain()
                .in_set(PhysicsSystems::Prepare),
        );
        app.add_systems(
            schedule,
            (
                mark_dirty_trees,
                propagate_parent_transforms,
                sync_simple_transforms,
            )
                .chain()
                .in_set(PhysicsTransformSystems::Propagate)
                .run_if(|config: Res<PhysicsTransformConfig>| config.propagate_before_physics),
        );
        app.add_systems(
            schedule,
            transform_to_position
                .in_set(PhysicsTransformSystems::TransformToPosition)
                .run_if(|config: Res<PhysicsTransformConfig>| config.transform_to_position),
        );
    }

    fn sync_position_to_transform(app: &mut App, schedule: impl ScheduleLabel) {
        app.init_resource::<PhysicsTransformConfig>();
        if app
            .world()
            .resource::<PhysicsTransformConfig>()
            .position_to_transform
        {
            app.register_required_components::<Position, Transform>();
            app.register_required_components::<Rotation, Transform>();
        }
        let schedule = schedule.intern();
        app.configure_sets(
            schedule,
            PhysicsTransformSystems::PositionToTransform.in_set(PhysicsSystems::Writeback),
        );
        app.add_systems(
            schedule,
            position_to_transform
                .in_set(PhysicsTransformSystems::PositionToTransform)
                .run_if(|config: Res<PhysicsTransformConfig>| config.position_to_transform),
        );
    }
}