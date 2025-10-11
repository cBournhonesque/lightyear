use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel};
use bevy_transform::systems::{
    mark_dirty_trees, propagate_parent_transforms, sync_simple_transforms,
};
use bevy_transform::{TransformSystems, components::Transform};
#[cfg(all(feature = "2d", not(feature = "3d")))]
use {
    crate::correction_2d as correction,
    avian2d::{
        dynamics::solver::{
            constraint_graph::ConstraintGraph,
            islands::{BodyIslandNode, PhysicsIslands},
        },
        physics_transform::*,
        prelude::*,
    },
};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use {
    crate::correction_3d as correction,
    avian3d::{
        dynamics::solver::{
            constraint_graph::ConstraintGraph,
            islands::{BodyIslandNode, PhysicsIslands},
        },
        physics_transform::*,
        prelude::*,
    },
};

use lightyear_frame_interpolation::FrameInterpolationSet;
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_prediction::plugin::PredictionSet;
use lightyear_prediction::prelude::{PredictionAppRegistrationExt, RollbackSet};
use lightyear_replication::prelude::TransformLinearInterpolation;

/// Indicate which components you are replicating over the network
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AvianReplicationMode {
    /// Replicate the Position component.
    /// PredictionHistory, Correction and FrameInterpolation also apply to Position.
    /// Physics updates must be applied directly to Position, NOT to Transform.
    #[default]
    Position,
    /// Replicate the Position component.
    /// Prediction is done on Position, but Correction and FrameInterpolation apply on Transform.
    ///
    /// This is because:
    /// - Position/Rotation are smaller to serialize and store
    /// - Correction/FrameInterpolation are a visual component so should operate on Transform
    PositionButInterpolateTransform,
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
    /// If True, the plugin will rollback island-related resources and components
    /// Enable this if you have the Island plugin enabled.
    pub rollback_islands: bool,
}

impl Plugin for LightyearAvianPlugin {
    fn build(&self, app: &mut App) {
        match self.replication_mode {
            AvianReplicationMode::Position => {
                // I think Transform to Position is updating Position to 0.0 ?

                if !self.update_syncs_manually {
                    // TODO: I think we should disable TranformToPosition, otherwise the FrameInterpolation::Restore will restore the correct Position,
                    //  but TransformToPosition might overwrite it!

                    // TODO: causes issues; for example in case a rollback fixes Position, this would reset the Position to the Transform! (if no
                    //  FrameInterpolation is enabled)
                    // LightyearAvianPlugin::sync_transform_to_position(app, RunFixedMainLoop);

                    // In case we do the TransformToPosition sync in RunFixedMainLoop, do it BEFORE
                    // restoring the correct Position in FrameInterpolation::Restore, since we want Position to take priority.
                    //
                    // TransformToPosition might be useful for child entities that need Transform->Position propagated.
                    app.configure_sets(
                        RunFixedMainLoop,
                        PhysicsSystems::Prepare
                            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                            .before(FrameInterpolationSet::Restore),
                    );
                    LightyearAvianPlugin::sync_position_to_transform(app, PostUpdate);

                    // TODO: it seems like if we apply TransformToPosition in FixedPostUpdate, we need
                    //  to also run PositionToTransform in FixedPostUpdate; how come?
                    //  is something modifying Transform in FixedUpdate and re-forcing a sync from Transform to Position, which overwrites
                    //
                    // LightyearAvianPlugin::sync_transform_to_position(app, FixedPostUpdate);
                    // LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
                }

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
            }
            AvianReplicationMode::PositionButInterpolateTransform => {
                // add custom correction systems
                app.add_systems(
                    PreUpdate,
                    correction::update_frame_interpolation_post_rollback
                        .in_set(RollbackSet::EndRollback),
                );
                app.add_systems(
                    PostUpdate,
                    correction::add_visual_correction.in_set(RollbackSet::VisualCorrection),
                );

                if !self.update_syncs_manually {
                    LightyearAvianPlugin::sync_transform_to_position(app, RunFixedMainLoop);
                    // In case we do the TransformToPosition sync in RunFixedMainLoop, do it AFTER
                    // restoring the correct Transform in FrameInterpolation::Restore
                    app.configure_sets(
                        RunFixedMainLoop,
                        PhysicsSystems::Prepare
                            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                            .after(FrameInterpolationSet::Restore),
                    );
                    LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
                }

                app.configure_sets(
                    FixedPostUpdate,
                    (
                        // update physics before we store the new Position in the history
                        (PhysicsSystems::StepSimulation, PredictionSet::UpdateHistory).chain(),
                        // make sure that the Transform has been updated before updating FrameInterpolation<Transform>
                        (PhysicsSystems::Writeback, FrameInterpolationSet::Update).chain(),
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
                if !self.update_syncs_manually {
                    // need to run TransformToPosition in FixedPostUpdate since avian uses Position internally
                    // but the user operates on Transform
                    LightyearAvianPlugin::sync_transform_to_position(app, FixedPostUpdate);
                    LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
                }
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
            app.init_resource::<ConstraintGraph>();
            app.add_resource_rollback::<ContactGraph>();
            app.add_resource_rollback::<ConstraintGraph>();
            app.add_rollback::<CollidingEntities>();

            if self.rollback_islands {
                app.init_resource::<PhysicsIslands>();
                app.add_resource_rollback::<PhysicsIslands>();
                app.add_rollback::<BodyIslandNode>();
                app.add_rollback::<Sleeping>();
            }
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
            // Make sure that PositionToTransform sync also runs for Interpolated entities
            app.register_required_components::<Position, ApplyPosToTransform>();
            app.register_required_components::<Rotation, ApplyPosToTransform>();

            // TODO(important): handle this
            // NOTE: we do NOT include this because Position/Rotation might not be added at the same time (for example on the Interpolated entity)
            //  we only want to add Transform if both are added at the same time
            // app.try_register_required_components::<Position, Transform>().ok();
            // app.try_register_required_components::<Rotation, Transform>().ok();
            app.add_observer(Self::position_rotation_to_transform);
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

    /// Add Transform only when Position/Rotation are both present.
    fn position_rotation_to_transform(
        trigger: On<Add, (Position, Rotation)>,
        query: Query<(), (With<Position>, With<Rotation>)>,
        mut commands: Commands,
    ) {
        if query.get(trigger.entity).is_ok() {
            // the Transform will be updated by the sync system
            commands
                .entity(trigger.entity)
                .insert(Transform::default());
        }
    }
}
