#[cfg(all(feature = "2d", not(feature = "3d")))]
use avian2d::{
    dynamics::solver::constraint_graph::ConstraintGraph, physics_transform::*, prelude::*,
};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use avian3d::{
    dynamics::solver::constraint_graph::ConstraintGraph, physics_transform::*, prelude::*,
};
use bevy_app::prelude::*;
use bevy_ecs::change_detection::Res;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel};
use bevy_transform::systems::{
    mark_dirty_trees, propagate_parent_transforms, sync_simple_transforms,
};
use bevy_transform::{TransformSystems, components::Transform};

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
    ///
    /// This can be useful to reduce the network bandwidth, but applying FrameInterpolation on Transform.
    PositionButPredictTransform,
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
                LightyearAvianPlugin::sync_transform_to_position(app, FixedPostUpdate);
                LightyearAvianPlugin::sync_position_to_transform(app, PostUpdate);
                app.configure_sets(
                    FixedPostUpdate,
                    // update physics before we store the new Position in the history
                    (
                        PhysicsSystems::StepSimulation,
                        (PredictionSet::UpdateHistory, FrameInterpolationSet::Update),
                    )
                        .chain(),
                );
                app.configure_sets(
                    PostUpdate,
                    (
                        FrameInterpolationSet::Interpolate,
                        // We don't want the correction to be overwritten by FrameInterpolation
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
            AvianReplicationMode::PositionButPredictTransform => {
                // - PreUpdate: we receive Confirmed<Position>
                //    - we need to convert this to Confirmed<Transform> before RollbackCheck
                //      this can be done with a custom Replicate fn that handles replicating
                //      both Position and Rotation together?
                // - Rollback:
                //    - a Correction<Transform> is applied
                // - FixedPostUpdate:
                //    - TransformToPosition
                //    - StepSimulation
                //    - PositionToTransform
                //    - (UpdateHistory, FrameInterpolatonSet)
                unimplemented!(
                    "Need to implement sync from Confirmed<Position> to Confirmed<Transform>"
                );

                // The main issue with this mode is that the Transform component gets replicated, but avian internally works on Position and Rotation components. So we need
                // to ensure that in PreUpdate, after receiving the Transform component, we sync it to Position and Rotation.
                // PreUpdate:
                // - we need an initial Sync from Transform to Position in PreUpdate
                // - we need to still do the rollback check on Position so the Position should be a non-networked component with PredictionMode = Full?
                // FixedPostUpdate:
                // - we need a sync from Position to Transform in FixedPostUpdate
                // Avian doesn't support updating the sync config separately for two schedules.

                LightyearAvianPlugin::sync_transform_to_position(app, FixedPostUpdate);
                LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
                app.configure_sets(
                    PreUpdate,
                    (
                        ReplicationSet::Receive,
                        // TODO: sync Confirmed<Position> to Confirmed<Transform>
                        RollbackSet::Check,
                    )
                        .chain(),
                );
                app.configure_sets(
                    FixedPostUpdate,
                    // update physics before we store the new Position in the history
                    (
                        PhysicsSystems::Prepare,
                        PhysicsSystems::StepSimulation,
                        PhysicsSystems::Writeback,
                        (PredictionSet::UpdateHistory, FrameInterpolationSet::Update),
                    )
                        .chain(),
                );
                app.configure_sets(
                    PostUpdate,
                    (
                        FrameInterpolationSet::Interpolate,
                        // We don't want the correction to be overwritten by FrameInterpolation
                        RollbackSet::VisualCorrection,
                        TransformSystems::Propagate,
                    )
                        .chain(),
                );
                // Even if we don't replicate Transform, we need to register an interpolation function
                // for it so that we can do frame interpolation
                app.world_mut()
                    .resource_mut::<InterpolationRegistry>()
                    // TODO: allow adding an interpolation function without replicating the component
                    //  or doing interpolation! That interpolation can be shared for the purposes of
                    //  frame_interpolation, correction, interpolation
                    .set_interpolation::<Transform>(TransformLinearInterpolation::lerp);
            }
            AvianReplicationMode::Transform => {
                LightyearAvianPlugin::sync_transform_to_position(app, FixedPostUpdate);
                LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
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
                        // sync updated Position to Transform
                        PhysicsSystems::Writeback,
                        (
                            // save the new Transform values in the history
                            PredictionSet::UpdateHistory,
                            // save the values for visual interpolation
                            FrameInterpolationSet::Update,
                        ),
                    )
                        .chain(),
                );
                app.configure_sets(
                    PostUpdate,
                    (
                        FrameInterpolationSet::Interpolate,
                        // We don't want the correction to be overwritten by FrameInterpolation
                        RollbackSet::VisualCorrection,
                        TransformSystems::Propagate,
                    )
                        .chain(),
                );
            }
        }

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
            // TODO(important): handle this
            // app.register_required_components::<Position, Transform>();
            // app.register_required_components::<Rotation, Transform>();
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
