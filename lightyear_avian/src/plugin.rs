/*!
Helpers to network avian components.

Some subtle footguns with avian replication:
- for Predicted entities, your `Position` is replicated as `Confirmed<Position>`. This triggers an immediate
  rollback on the client which inserts the correct `Position`.
- for `Interpolated` entities, it is possible that only one of `Position` or `Rotation` gets added
  (and not both at the same time). This can happen if Rotation doesn't get updated frequently for your
  entity, since we insert the real component only after receiving two remote updates. This can cause
  issues because the `sync_pos_to_transform` system from avian only does the sync from
  `Position/Rotation` -> `Transform` when BOTH are present on the same time. So you might be stuck with
  a `Transform::default()` for a short-while, until both Position/Rotation are present on the
  entity. For that reason it's best to add rendering components on `Interpolated` entities only when
  BOTH Position and Rotation are present.
- Inserting `RigidBody` on an entity automatically inserts Position/Rotation/Transform on it. For that reason
  you do NOT want to add `RigidBody` on interpolated entities because it's going to display the entity at
  `Transform::default()` until the first interpolation updates are received. (And also because you don't
  want any avian systems to run for `Interpolated` entities)
- Do not forget to disable some of the avian plugins!
```rust,ignore
PhysicsPlugins::default()
    .build()
    // disable the position<>transform sync plugins as it is handled by lightyear_avian
    .disable::<PhysicsTransformPlugin>()
    // FrameInterpolation handles interpolating Position and Rotation
    .disable::<PhysicsInterpolationPlugin>()
    // disable island plugins as it can mess with rollbacks. Only if you're doing deterministic replication.
    // For state replication it should be fine to keep them.
    // .disable::<IslandPlugin>()
    // .disable::<IslandSleepingPlugin>(),
```
!*/
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel};
use bevy_transform::components::GlobalTransform;
use bevy_transform::systems::{
    mark_dirty_trees, propagate_parent_transforms, sync_simple_transforms,
};
use bevy_transform::{TransformSystems, components::Transform};
#[allow(unused_imports)]
use tracing::info;
use tracing::trace;
#[cfg(all(feature = "2d", not(feature = "3d")))]
use {
    crate::correction_2d as correction,
    avian2d::{
        dynamics::solver::{
            constraint_graph::ConstraintGraph,
            islands::{BodyIslandNode, PhysicsIslands},
        },
        math::*,
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
        math::*,
        physics_transform::*,
        prelude::*,
    },
};

use lightyear_frame_interpolation::FrameInterpolationSystems;
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_prediction::plugin::PredictionSystems;
use lightyear_prediction::prelude::{PredictionAppRegistrationExt, RollbackSystems};
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
    /// I believe that this currently does NOT handle TransformPropagation to children correctly.
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
        app.init_resource::<PhysicsTransformConfig>();
        match self.replication_mode {
            AvianReplicationMode::Position => {
                if !self.update_syncs_manually {
                    // TODO: causes issues if no FrameInterpolation is enabled, because we don't override the transform->position with the correct Position
                    //  (for example in case a rollback updates Position, that change will be overridden by the transform->position)
                    LightyearAvianPlugin::sync_transform_to_position(app, RunFixedMainLoop);

                    // In case we do the TransformToPosition sync in RunFixedMainLoop, do it BEFORE
                    // restoring the correct Position in FrameInterpolation::Restore, since we want Position to take priority.
                    //
                    // TransformToPosition might be useful for child entities that need Transform->Position propagated.
                    app.configure_sets(
                        RunFixedMainLoop,
                        PhysicsSystems::Prepare
                            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                            .before(FrameInterpolationSystems::Restore),
                    );
                    LightyearAvianPlugin::sync_position_to_transform(app, PostUpdate);

                    // TODO: it seems like if we apply TransformToPosition in FixedPostUpdate, we need
                    //  to also run PositionToTransform in FixedPostUpdate; how come?
                    //  is something modifying Transform in FixedUpdate and re-forcing a sync from Transform to Position, which overwrites
                    //
                    // LightyearAvianPlugin::sync_transform_to_position(app, FixedPostUpdate);
                    // LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
                }
                // We need to manually update the Position of child colliders after physics run
                // since avian doesn't do it
                app.add_systems(
                    RunFixedMainLoop,
                    LightyearAvianPlugin::update_child_collider_position
                        .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
                );

                app.configure_sets(
                    FixedPostUpdate,
                    // update physics before we store the new Position in the history
                    (
                        PhysicsSystems::StepSimulation,
                        (
                            PredictionSystems::UpdateHistory,
                            FrameInterpolationSystems::Update,
                        ),
                    )
                        .chain(),
                );
                app.configure_sets(
                    PostUpdate,
                    (
                        FrameInterpolationSystems::Interpolate,
                        // We don't want the correction to be overwritten by FrameInterpolation
                        RollbackSystems::VisualCorrection,
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
                        .in_set(RollbackSystems::EndRollback),
                );
                app.add_systems(
                    PostUpdate,
                    correction::add_visual_correction.in_set(RollbackSystems::VisualCorrection),
                );

                if !self.update_syncs_manually {
                    LightyearAvianPlugin::sync_transform_to_position(app, RunFixedMainLoop);
                    // In case we do the TransformToPosition sync in RunFixedMainLoop, do it AFTER
                    // restoring the correct Transform in FrameInterpolation::Restore
                    app.configure_sets(
                        RunFixedMainLoop,
                        PhysicsSystems::Prepare
                            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                            .after(FrameInterpolationSystems::Restore),
                    );
                    LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
                }
                // We need to manually update the Position of child colliders after physics run
                // since avian doesn't do it.
                // Runs after physics because the Parent's Position must be updated.
                app.add_systems(
                    RunFixedMainLoop,
                    LightyearAvianPlugin::update_child_collider_position
                        .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
                );

                app.configure_sets(
                    FixedPostUpdate,
                    (
                        // update physics before we store the new Position in the history
                        (
                            PhysicsSystems::StepSimulation,
                            PredictionSystems::UpdateHistory,
                        )
                            .chain(),
                        // make sure that the Transform has been updated before updating FrameInterpolation<Transform>
                        (PhysicsSystems::Writeback, FrameInterpolationSystems::Update).chain(),
                    )
                        .chain(),
                );
                app.configure_sets(
                    PostUpdate,
                    (
                        FrameInterpolationSystems::Interpolate,
                        // We don't want the correction to be overwritten by FrameInterpolation
                        RollbackSystems::VisualCorrection,
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
                    // make sure the child collider's position is updated before running
                    // PositionToTransform (otherwise the child's Position would not be correct
                    // when running PositionToTransform)
                    app.add_systems(
                        FixedPostUpdate,
                        LightyearAvianPlugin::update_child_collider_position
                            .in_set(PhysicsTransformSystems::PositionToTransform)
                            .before(position_to_transform),
                    );
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
                            PredictionSystems::UpdateHistory,
                            // save the values for visual interpolation
                            FrameInterpolationSystems::Update,
                        ),
                    )
                        .chain(),
                );
                app.configure_sets(
                    PostUpdate,
                    (
                        FrameInterpolationSystems::Interpolate,
                        // We don't want the correction to be overwritten by FrameInterpolation
                        RollbackSystems::VisualCorrection,
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
        let schedule = schedule.intern();
        // also add the system ordering for FixedPostUpdate (for ColliderTransformPlugin)
        app.configure_sets(
            FixedPostUpdate,
            (
                PhysicsTransformSystems::Propagate,
                PhysicsTransformSystems::TransformToPosition,
            )
                .chain()
                .in_set(PhysicsSystems::Prepare),
        );
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
        }
        let schedule = schedule.intern();

        // TODO: do we need to add this in PreUpdate to avoid 1-frame delays?
        // app.add_systems(PreUpdate, Self::position_rotation_to_transform
        //     .after(ReplicationSystems::Receive));

        app.configure_sets(
            FixedPostUpdate,
            PhysicsTransformSystems::PositionToTransform.in_set(PhysicsSystems::Writeback),
        );
        app.configure_sets(
            schedule,
            PhysicsTransformSystems::PositionToTransform.in_set(PhysicsSystems::Writeback),
        );
        // app.add_observer(Self::add_transform);
        app.add_systems(
            schedule,
            (position_to_transform, Self::add_transform)
                .in_set(PhysicsTransformSystems::PositionToTransform)
                .run_if(|config: Res<PhysicsTransformConfig>| config.position_to_transform),
        );
    }

    // /// Add Transform only when Position/Rotation are both present and Transform is not.
    // /// This is necessary because the PositionToTransform systems require `Transform`.
    // ///
    // /// Note, this is will only work is `ChildOf` is inserted at the same time or before
    // /// `Position/Rotation`.
    // fn add_transform(
    //     trigger: On<Add, (Position, Rotation)>,
    //     query: Query<(&Position, &Rotation, Option<&ChildOf>), Without<Transform>>,
    //     parents: Query<(
    //         Option<&GlobalTransform>,
    //         Option<&Position>,
    //         Option<&Rotation>,
    //     )>,
    //     mut commands: Commands,
    // ) {
    //     let entity = trigger.entity;
    //     if let Ok((pos, rot, parent)) = query.get(entity) {
    //         let mut transform = Transform::default();
    //         #[cfg(feature = "2d")]
    //         if let Some(&ChildOf(parent)) = parent {
    //             if let Ok((parent_global_transform, parent_pos, parent_rot)) = parents.get(parent) {
    //                 // Compute the global transform of the parent using its Position and Rotation
    //                 let parent_transform = parent_global_transform
    //                     .unwrap_or(&GlobalTransform::IDENTITY)
    //                     .compute_transform();
    //                 let parent_pos = parent_pos.map_or(parent_transform.translation, |pos| {
    //                     pos.f32().extend(parent_transform.translation.z)
    //                 });
    //                 let parent_rot = parent_rot.map_or(parent_transform.rotation, |rot| {
    //                     Quaternion::from(*rot).f32()
    //                 });
    //                 let parent_scale = parent_transform.scale;
    //                 let parent_transform = Transform::from_translation(parent_pos)
    //                     .with_rotation(parent_rot)
    //                     .with_scale(parent_scale);
    //
    //                 // The new local transform of the child body,
    //                 // computed from the its global transform and its parents global transform
    //                 let new_transform = GlobalTransform::from(
    //                     Transform::from_translation(
    //                         pos.f32().extend(parent_transform.translation.z),
    //                     )
    //                     .with_rotation(Quaternion::from(*rot).f32()),
    //                 )
    //                 .reparented_to(&GlobalTransform::from(parent_transform));
    //
    //                 transform.translation = new_transform.translation;
    //                 transform.rotation = new_transform.rotation;
    //             }
    //         } else {
    //             transform.translation = pos.f32().extend(transform.translation.z);
    //             transform.rotation = Quaternion::from(*rot).f32();
    //         }
    //
    //         #[cfg(feature = "3d")]
    //         if let Some(&ChildOf(parent)) = parent {
    //             if let Ok((parent_global_transform, parent_pos, parent_rot)) = parents.get(parent) {
    //                 // Compute the global transform of the parent using its Position and Rotation
    //                 let parent_transform = parent_global_transform
    //                     .unwrap_or(&GlobalTransform::IDENTITY)
    //                     .compute_transform();
    //                 let parent_pos =
    //                     parent_pos.map_or(parent_transform.translation, |pos| pos.f32());
    //                 let parent_rot = parent_rot.map_or(parent_transform.rotation, |rot| rot.f32());
    //                 let parent_scale = parent_transform.scale;
    //                 let parent_transform = Transform::from_translation(parent_pos)
    //                     .with_rotation(parent_rot)
    //                     .with_scale(parent_scale);
    //
    //                 // The new local transform of the child body,
    //                 // computed from the its global transform and its parents global transform
    //                 let new_transform = GlobalTransform::from(
    //                     Transform::from_translation(pos.f32()).with_rotation(rot.f32()),
    //                 )
    //                 .reparented_to(&GlobalTransform::from(parent_transform));
    //
    //                 transform.translation = new_transform.translation;
    //                 transform.rotation = new_transform.rotation;
    //             }
    //         } else {
    //             transform.translation = pos.f32();
    //             transform.rotation = rot.f32();
    //         }
    //
    //         trace!(
    //             ?transform,
    //             "Adding transform because Position/Rotation were added for {entity:?}"
    //         );
    //         commands.entity(entity).insert(transform);
    //     };
    // }

    /// Add Transform only when Position/Rotation are both present and Transform is not.
    /// This is necessary because the PositionToTransform systems require `Transform`
    ///
    /// - We cannot run this as an observer because the `ChildOf` component might be inserted
    ///   after Position/Rotation.
    /// - We cannot add Transform::default because if the entity is spawned in PreUpdate,
    ///   the TransformToPosition will overwrite the correct Position/Rotation.
    /// - We cannot just add GlobalTransform because the PositionToTransform systems requires the
    ///   `Transform` component to be present
    /// - Therefore we try to compute the correct `Transform`
    fn add_transform(
        query: Query<(Entity, Ref<Position>, Ref<Rotation>, Option<&ChildOf>), Without<Transform>>,
        parents: Query<(
            Option<&GlobalTransform>,
            Option<&Position>,
            Option<&Rotation>,
        )>,
        mut commands: Commands,
    ) {
        query.iter().for_each(|(entity, pos, rot, parent)| {
            if !(pos.is_added() || rot.is_added()) {
                return;
            }
            let mut transform = Transform::default();
            #[cfg(feature = "2d")]
            if let Some(&ChildOf(parent)) = parent {
                if let Ok((parent_global_transform, parent_pos, parent_rot)) = parents.get(parent) {
                    // Compute the global transform of the parent using its Position and Rotation
                    let parent_transform = parent_global_transform
                        .unwrap_or(&GlobalTransform::IDENTITY)
                        .compute_transform();
                    let parent_pos = parent_pos.map_or(parent_transform.translation, |pos| {
                        pos.f32().extend(parent_transform.translation.z)
                    });
                    let parent_rot = parent_rot.map_or(parent_transform.rotation, |rot| {
                        Quaternion::from(*rot).f32()
                    });
                    let parent_scale = parent_transform.scale;
                    let parent_transform = Transform::from_translation(parent_pos)
                        .with_rotation(parent_rot)
                        .with_scale(parent_scale);

                    // The new local transform of the child body,
                    // computed from the its global transform and its parents global transform
                    let new_transform = GlobalTransform::from(
                        Transform::from_translation(
                            pos.f32().extend(parent_transform.translation.z),
                        )
                        .with_rotation(Quaternion::from(*rot).f32()),
                    )
                    .reparented_to(&GlobalTransform::from(parent_transform));

                    transform.translation = new_transform.translation;
                    transform.rotation = new_transform.rotation;
                }
            } else {
                transform.translation = pos.f32().extend(transform.translation.z);
                transform.rotation = Quaternion::from(*rot).f32();
            }

            #[cfg(feature = "3d")]
            if let Some(&ChildOf(parent)) = parent {
                if let Ok((parent_global_transform, parent_pos, parent_rot)) = parents.get(parent) {
                    // Compute the global transform of the parent using its Position and Rotation
                    let parent_transform = parent_global_transform
                        .unwrap_or(&GlobalTransform::IDENTITY)
                        .compute_transform();
                    let parent_pos =
                        parent_pos.map_or(parent_transform.translation, |pos| pos.f32());
                    let parent_rot = parent_rot.map_or(parent_transform.rotation, |rot| rot.f32());
                    let parent_scale = parent_transform.scale;
                    let parent_transform = Transform::from_translation(parent_pos)
                        .with_rotation(parent_rot)
                        .with_scale(parent_scale);

                    // The new local transform of the child body,
                    // computed from the its global transform and its parents global transform
                    let new_transform = GlobalTransform::from(
                        Transform::from_translation(pos.f32()).with_rotation(rot.f32()),
                    )
                    .reparented_to(&GlobalTransform::from(parent_transform));

                    transform.translation = new_transform.translation;
                    transform.rotation = new_transform.rotation;
                }
            } else {
                transform.translation = pos.f32();
                transform.rotation = rot.f32();
            }

            trace!(
                ?transform,
                "Adding transform because Position/Rotation were added for {entity:?}"
            );
            commands.entity(entity).insert(transform);
        });
    }

    /// Update the child's Position based on the paren't Position and the child's Transform.
    ///
    /// In avian, this is done in PhysicsSystems::First, so we need to manually run it
    /// after PhysicsSystems run to have an accurate Position of child entities
    /// for replication
    #[allow(clippy::type_complexity)]
    pub fn update_child_collider_position(
        mut collider_query: Query<
            (
                &ColliderTransform,
                &mut Position,
                &mut Rotation,
                &ColliderOf,
            ),
            Without<RigidBody>,
        >,
        rb_query: Query<(&Position, &Rotation), (With<RigidBody>, With<Children>)>,
    ) {
        for (collider_transform, mut position, mut rotation, collider_of) in &mut collider_query {
            let Ok((rb_pos, rb_rot)) = rb_query.get(collider_of.body) else {
                continue;
            };

            position.0 = rb_pos.0 + rb_rot * collider_transform.translation;
            #[cfg(feature = "2d")]
            {
                *rotation = *rb_rot * collider_transform.rotation;
            }
            #[cfg(feature = "3d")]
            {
                *rotation = (rb_rot.0 * collider_transform.rotation.0)
                    .normalize()
                    .into();
            }
        }
    }
}
