/*!
Helpers to network avian components.

Some subtle footguns with avian replication:
- [`AvianReplicationMode::Position`] is the preferred mode. `Position` and `Rotation` are the
  canonical simulation state. By default, synchronization is one-way from those components to
  `Transform`, so gameplay should move physics bodies through `Position` and `Rotation`.
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
```

## Position mode scheduling

When frame interpolation and correction are enabled, the important ordering is:

```text
PreUpdate:
  receive replication -> rollback and replay -> repair frame history

RunFixedMainLoop (before the fixed loop):
  restore canonical Position/Rotation
  -> optionally canonicalize Transform for transform-driven FixedUpdate gameplay

FixedPostUpdate (for every fixed tick):
  optionally import an authored Transform
  -> Avian physics
  -> optionally copy Position/Rotation back to Transform
  -> prediction history + frame history

PostUpdate:
  frame-interpolate Position/Rotation
  -> apply visual correction to Position/Rotation
  -> copy changed Position/Rotation to Transform
  -> propagate Transform
```

The restore step prevents the visual values written in the previous `PostUpdate` from entering
the next simulation tick. Transform-to-position synchronization is disabled by default, so a stale
`Transform` cannot overwrite rollback-corrected physics state. Set
`AvianReplicationMode::Position { sync_to_transform: true }` only when gameplay intentionally
authors `Transform` during fixed ticks.

Visual correction uses frame-interpolation rules and history. Registering correction with
`add_correction` installs the frame-interpolation plugin automatically, and rollback adds
[`FrameInterpolate`](lightyear_frame_interpolation::FrameInterpolate) when it stores the previous
visual value. If neither visual feature is wanted, omit correction; rollback then snaps directly
to canonical physics state.

Position mode automatically registers velocity-aware Hermite interpolation for the
`(Position, Rotation, LinearVelocity, AngularVelocity)` bundle. Delayed interpolation, frame
interpolation, and visual correction select it whenever all four components are present. The
per-component rules remain useful for archetypes that do not contain the complete bundle.

By default, Position mode also registers `Position`, `Rotation`, `LinearVelocity`, and
`AngularVelocity` for filtered rigid-body replication and prediction. Position and Rotation get
linear interpolation, correction, and a small default rollback tolerance. Set
[`LightyearAvianPlugin::register_physics_components`] to `false` when the application owns these
registrations, for example to use
`app.component::<C>().replicate().predict().with_rollback_condition(...)` or deterministic
input-only replication.
!*/
#[cfg(all(feature = "2d", not(feature = "3d")))]
use avian2d::{math::*, physics_transform::*, prelude::*};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use avian3d::{math::*, physics_transform::*, prelude::*};
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

use lightyear_frame_interpolation::FrameInterpolationSystems;
use lightyear_interpolation::prelude::{
    AppInterpolationExt, Interpolated, InterpolationFns, InterpolationRegistrationExt,
};
use lightyear_prediction::plugin::PredictionSystems;
use lightyear_prediction::prelude::{PredictionBuilderExt, RollbackSystems};
use lightyear_replication::prelude::{
    AppComponentExt, ComponentRegistry, TransformLinearInterpolation,
};

/// Indicate which components you are replicating over the network
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AvianReplicationMode {
    /// Replicate [`Position`] and [`Rotation`].
    ///
    /// This is the preferred mode for Avian physics. Prediction history, correction, and frame
    /// interpolation apply to `Position` and `Rotation`, and the rendered [`Transform`] is written
    /// from that visual physics state in `PostUpdate`.
    ///
    /// Child colliders without their own rigid body still use a local `Transform` to describe
    /// their offset from the parent body.
    Position {
        /// If `true`, synchronize `Position` and `Rotation` to `Transform` before `FixedUpdate`, so
        /// gameplay can safely read and update `Transform` there. The authored transform is then
        /// imported into `Position` and `Rotation` before Avian physics runs.
        ///
        /// This defaults to `false`, keeping physics authority strictly one-way from `Position` and
        /// `Rotation` to `Transform`.
        sync_to_transform: bool,
    },
    /// Replicate [`Transform`].
    ///
    /// Prediction history, correction, and frame interpolation also apply to `Transform`. Use this
    /// when application code intentionally treats transforms as authoritative—for example, a
    /// transform-driven kinematic or animation pipeline—or when local transform data such as scale
    /// is part of the replicated gameplay state. Avian still simulates with `Position` and
    /// `Rotation`, so this mode requires synchronization in both directions.
    Transform,
}

impl Default for AvianReplicationMode {
    fn default() -> Self {
        Self::Position {
            sync_to_transform: false,
        }
    }
}

/// Plugin that integrates Avian with Lightyear for networked physics replication.
///
/// NOTE: this plugin is NOT added automatically by ClientPlugins/ServerPlugins, you have to add it manually!
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightyearAvianPlugin {
    /// The replication mode to use for the Avian plugin.
    pub replication_mode: AvianReplicationMode,
    /// If true, Lightyear does not install automatic `Position`/`Rotation`/`Transform` sync systems.
    ///
    /// Enable this if you are an advanced user and want to handle all synchronization manually.
    pub update_syncs_manually: bool,
    /// Register the standard Position-mode Avian components for networking.
    ///
    /// The default registration replicates `Position`, `Rotation`, `LinearVelocity`, and
    /// `AngularVelocity` only on entities with [`RigidBody`], predicts all four components, and
    /// enables interpolation and correction for the pose components. Disable this when the
    /// application uses custom replication rules, such as replicate-once deterministic state.
    ///
    /// Add the Lightyear networking plugins or another component protocol before this plugin so
    /// the replication registry exists when these defaults are installed.
    pub register_physics_components: bool,
    /// Roll back Avian's persistent derived simulation state.
    ///
    /// This includes contact and constraint graphs (including cached impulses), the joint graph,
    /// persistent collider-tree and moved-proxy state, collider AABBs and proxy keys, the
    /// physics/substep clocks, joint-motor warm-start state when the integration's `xpbd_joints`
    /// feature is enabled
    /// (`avian_xpbd_joints` on the `lightyear` facade), and optional island/sleeping state.
    /// `CollidingEntities` is rebuilt from the restored contact graph before replay.
    ///
    /// Enable this for deterministic or input-only replication. Persistent collider-tree data is
    /// cloned into prediction history every tick, which can still be expensive for very large
    /// scenes. Avian's collider-tree workspaces are scratch allocation caches and are excluded from
    /// history. The integration uses a custom snapshot only for that exclusion; directly storing
    /// `ColliderTrees` and `MovedProxies` would otherwise provide equivalent rollback state.
    ///
    /// This does not register authoritative body or application-owned state such as `RigidBody`,
    /// `Position`, `Rotation`, velocities, forces, impulses, collider/joint configuration, or
    /// gameplay side effects. Register or deterministically recreate every such value that can
    /// change during predicted simulation. Collision events can be emitted again during replay, so
    /// irreversible event consumers must also be rollback-aware.
    ///
    /// If Avian's `IslandPlugin` is enabled, island rollback state is registered automatically
    /// during `finish()`. If `IslandSleepingPlugin` is also enabled, sleeping state is rolled back too.
    pub rollback_resources: bool,
}

impl Default for LightyearAvianPlugin {
    fn default() -> Self {
        Self {
            replication_mode: AvianReplicationMode::default(),
            update_syncs_manually: false,
            register_physics_components: true,
            rollback_resources: false,
        }
    }
}

const DEFAULT_ROLLBACK_TOLERANCE: f32 = 0.01;

fn position_should_rollback(confirmed: &Position, predicted: &Position) -> bool {
    (confirmed.0 - predicted.0).length() >= Scalar::from(DEFAULT_ROLLBACK_TOLERANCE)
}

fn rotation_should_rollback(confirmed: &Rotation, predicted: &Rotation) -> bool {
    confirmed.angle_between(*predicted) >= Scalar::from(DEFAULT_ROLLBACK_TOLERANCE)
}

fn linear_velocity_should_rollback(confirmed: &LinearVelocity, predicted: &LinearVelocity) -> bool {
    (confirmed.0 - predicted.0).length() >= Scalar::from(DEFAULT_ROLLBACK_TOLERANCE)
}

#[cfg(all(feature = "2d", not(feature = "3d")))]
fn angular_velocity_should_rollback(
    confirmed: &AngularVelocity,
    predicted: &AngularVelocity,
) -> bool {
    (confirmed.0 - predicted.0).abs() >= Scalar::from(DEFAULT_ROLLBACK_TOLERANCE)
}

#[cfg(all(feature = "3d", not(feature = "2d")))]
fn angular_velocity_should_rollback(
    confirmed: &AngularVelocity,
    predicted: &AngularVelocity,
) -> bool {
    (confirmed.0 - predicted.0).length() >= Scalar::from(DEFAULT_ROLLBACK_TOLERANCE)
}

impl Plugin for LightyearAvianPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsTransformConfig>();
        Self::install_position_to_transform_markers(app);
        match self.replication_mode {
            AvianReplicationMode::Position { sync_to_transform } => {
                if self.register_physics_components
                    && app.world().contains_resource::<ComponentRegistry>()
                {
                    Self::register_position_mode_components(app);
                }
                Self::add_position_rotation_hermite_rule(app);
                if !self.update_syncs_manually {
                    let mut config = app.world_mut().resource_mut::<PhysicsTransformConfig>();
                    config.position_to_transform = true;
                    config.transform_to_position = sync_to_transform;

                    if sync_to_transform {
                        // `PostUpdate` leaves the interpolated/corrected visual pose in both
                        // Position/Rotation and Transform. FrameInterpolationSystems::Restore
                        // restores only the canonical Position/Rotation before the fixed loop, so
                        // Transform would otherwise still contain the previous frame's visual pose.
                        //
                        // PositionToTransform gives FixedUpdate a canonical local Transform to read
                        // and modify. Propagate is also needed here so GlobalTransform and the
                        // transform hierarchy agree with that canonical local Transform before any
                        // fixed-tick gameplay runs. These systems run once before the whole fixed
                        // loop; the FixedPostUpdate writeback below prepares Transform between
                        // multiple fixed ticks in the same rendered frame.
                        LightyearAvianPlugin::sync_position_to_transform(app, RunFixedMainLoop);
                        LightyearAvianPlugin::propagate_transform(app, RunFixedMainLoop);
                        app.configure_sets(
                            RunFixedMainLoop,
                            (
                                FrameInterpolationSystems::Restore,
                                PhysicsTransformSystems::PositionToTransform,
                                PhysicsTransformSystems::Propagate,
                            )
                                .chain()
                                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
                        );

                        // FixedUpdate is allowed to modify Transform in this mode. Propagation in
                        // the authoritative sync updates GlobalTransform from those local changes,
                        // then TransformToPosition imports the authored pose before Avian physics
                        // runs. After physics, PositionToTransform writes the simulated pose back
                        // so Transform is canonical for the next fixed tick.
                        LightyearAvianPlugin::sync_transform_to_position_authoritative(
                            app,
                            FixedPostUpdate,
                        );
                        LightyearAvianPlugin::sync_position_to_transform(app, FixedPostUpdate);
                    }

                    // This is the only Position -> Transform sync in the default configuration.
                    // It observes the final frame-interpolated and corrected physics pose.
                    LightyearAvianPlugin::sync_position_to_transform(app, PostUpdate);
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
                        PhysicsSystems::Writeback,
                        // Normally both histories read the same completed physics state, so their
                        // relative order does not matter. Still, applications may mutate physics
                        // state after the physics step and before prediction history is recorded
                        // (for example, by setting velocity to zero after a collision). Capture
                        // frame history afterward too, so Restore cannot reapply the pre-mutation
                        // value on the next frame.
                        PredictionSystems::UpdateHistory,
                        FrameInterpolationSystems::Update,
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
            AvianReplicationMode::Transform => {
                Self::add_transform_frame_interpolation_rule(app);
                if !self.update_syncs_manually {
                    // need to run TransformToPosition in FixedPostUpdate since avian uses Position internally
                    // but the user operates on Transform
                    LightyearAvianPlugin::install_transform_to_position_sync(app, FixedPostUpdate);
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
                        // Preserve the same post-physics application-system boundary as Position
                        // mode before recording values for frame interpolation.
                        PredictionSystems::UpdateHistory,
                        // save the values for visual interpolation
                        FrameInterpolationSystems::Update,
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

        // Avian's ColliderOf::on_insert requires GlobalTransform to set up
        // the RigidBodyColliders relationship. Since PhysicsTransformPlugin is disabled,
        // we register Transform as required for ColliderMarker so GlobalTransform is present
        // for any concrete collider backend, including builds without Avian's default Collider.
        #[cfg(all(feature = "3d", not(feature = "2d")))]
        app.try_register_required_components::<avian3d::prelude::ColliderMarker, Transform>()
            .ok();
        #[cfg(all(feature = "2d", not(feature = "3d")))]
        app.try_register_required_components::<avian2d::prelude::ColliderMarker, Transform>()
            .ok();

        if self.rollback_resources {
            crate::rollback::register_rollback(app);
        }
    }

    fn finish(&self, app: &mut App) {
        if self.rollback_resources && app.is_plugin_added::<IslandPlugin>() {
            let rollback_sleeping = app.is_plugin_added::<IslandSleepingPlugin>();
            crate::rollback::register_island_rollback(app, rollback_sleeping);
        }
    }
}

impl LightyearAvianPlugin {
    /// Register the standard network protocol for Avian's canonical Position-mode state.
    fn register_position_mode_components(app: &mut App) {
        app.component::<Position>()
            .replicate_filtered::<With<RigidBody>>()
            .predict()
            .with_rollback_condition(position_should_rollback)
            .add_linear_interpolation()
            .add_correction();

        app.component::<Rotation>()
            .replicate_filtered::<With<RigidBody>>()
            .predict()
            .with_rollback_condition(rotation_should_rollback)
            .add_linear_interpolation()
            .add_correction();

        app.component::<LinearVelocity>()
            .replicate_filtered::<With<RigidBody>>()
            .predict()
            .with_rollback_condition(linear_velocity_should_rollback);

        app.component::<AngularVelocity>()
            .replicate_filtered::<With<RigidBody>>()
            .predict()
            .with_rollback_condition(angular_velocity_should_rollback);
    }

    /// Opt non-rigid interpolated entities into Avian's Position-to-Transform sync.
    ///
    /// Avian's [`position_to_transform`] normally only synchronizes entities with a
    /// [`RigidBody`]. Lightyear's render-only [`Interpolated`] entities intentionally do not have
    /// a rigid body, but their sampled [`Position`] and [`Rotation`] still need to be written to
    /// [`Transform`] for rendering. [`ApplyPosToTransform`] opts those entities into Avian's
    /// system.
    ///
    /// A non-rigid child [`ColliderOf`] is different: its `Position` and `Rotation` are derived
    /// world state computed from the body root and its [`ColliderTransform`], while its
    /// `Transform` stores the fixed local offset from that root. Applying the derived world pose
    /// to the local transform would apply the hierarchy twice and move the collider away from the
    /// body. Transform synchronization must therefore start at the body root, and compound child
    /// colliders must not receive [`ApplyPosToTransform`].
    ///
    /// Avian can also give a collider root a self-referential [`ColliderOf`]. Such an entity is not
    /// a child collider: if it is [`Interpolated`] and has no [`RigidBody`], it still needs
    /// [`ApplyPosToTransform`] so its sampled pose reaches [`Transform`]. The observers therefore
    /// distinguish roots (`ColliderOf::body == entity`) from actual child colliders. The removal
    /// observer handles either component insertion order.
    fn install_position_to_transform_markers(app: &mut App) {
        app.add_observer(Self::add_apply_pos_to_transform);
        app.add_observer(Self::remove_apply_pos_to_transform_from_child_collider);
    }

    fn add_apply_pos_to_transform(
        trigger: On<Add, (Position, Rotation, Interpolated)>,
        query: Query<
            Option<&ColliderOf>,
            (
                With<Position>,
                With<Rotation>,
                With<Interpolated>,
                Without<ApplyPosToTransform>,
                Without<RigidBody>,
            ),
        >,
        mut commands: Commands,
    ) {
        if query.get(trigger.entity).is_ok_and(|collider_of| {
            collider_of.is_none_or(|collider_of| collider_of.body == trigger.entity)
        }) {
            commands.entity(trigger.entity).insert(ApplyPosToTransform);
        }
    }

    fn remove_apply_pos_to_transform_from_child_collider(
        trigger: On<Insert, (ColliderOf, ApplyPosToTransform)>,
        query: Query<
            &ColliderOf,
            (
                With<ColliderOf>,
                With<ApplyPosToTransform>,
                Without<RigidBody>,
            ),
        >,
        mut commands: Commands,
    ) {
        if query
            .get(trigger.entity)
            .is_ok_and(|collider_of| collider_of.body != trigger.entity)
        {
            commands
                .entity(trigger.entity)
                .remove::<ApplyPosToTransform>();
        }
    }

    /// Register the canonical Avian pose and velocity bundle once for every
    /// Position-mode application. The bundle's default priority is its four
    /// members, so it wins over ordinary single-component rules whenever the
    /// complete bundle is present.
    fn add_position_rotation_hermite_rule(app: &mut App) {
        app.interpolate_bundle_with::<(Position, Rotation, LinearVelocity, AngularVelocity)>(
            InterpolationFns::interpolate_with_context(crate::types::position_rotation::hermite),
        );
    }

    // Add a low-priority interpolation function for Transform
    // (that could be used for Correction or FrameInterpolation)
    fn add_transform_frame_interpolation_rule(app: &mut App) {
        app.interpolate_with_priority::<Transform>(
            0,
            InterpolationFns::no_history(TransformLinearInterpolation::lerp),
        );
    }

    fn propagate_transform(app: &mut App, schedule: impl ScheduleLabel) {
        let schedule = schedule.intern();
        app.configure_sets(
            schedule,
            PhysicsTransformSystems::Propagate.in_set(PhysicsSystems::Prepare),
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
    }

    fn install_transform_to_position_sync(app: &mut App, schedule: impl ScheduleLabel) {
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
        app.configure_sets(
            schedule,
            PhysicsTransformSystems::TransformToPosition
                .in_set(PhysicsSystems::Prepare)
                .after(PhysicsTransformSystems::Propagate),
        );
        Self::propagate_transform(app, schedule);
        app.add_systems(
            schedule,
            transform_to_position
                .in_set(PhysicsTransformSystems::TransformToPosition)
                .run_if(|config: Res<PhysicsTransformConfig>| config.transform_to_position),
        );
    }

    /// Install transform-authoritative synchronization for fixed-tick gameplay.
    ///
    /// Avian's normal `transform_to_position` gives precedence to a `Position` changed since the
    /// previous physics tick. Frame interpolation correctly advances `Position` change detection
    /// while writing the visual pose in `PostUpdate`, after the previous physics tick. Restoring
    /// the canonical value before the next fixed loop does not rewind that change tick. Avian's
    /// conflict rule would therefore reject even a `Transform` that the user authored afterward
    /// in `FixedUpdate`.
    ///
    /// `sync_to_transform: true` explicitly makes `Transform` the authoring API during
    /// `FixedUpdate`, so this variant deliberately gives the propagated transform precedence at
    /// the pre-physics boundary. Keeping this as a separate variant limits that override to this
    /// opt-in path; all other transform-to-position synchronization keeps Avian's normal conflict
    /// resolution.
    fn sync_transform_to_position_authoritative(app: &mut App, schedule: impl ScheduleLabel) {
        let schedule = schedule.intern();
        app.configure_sets(
            schedule,
            PhysicsTransformSystems::TransformToPosition
                .in_set(PhysicsSystems::Prepare)
                .after(PhysicsTransformSystems::Propagate),
        );
        Self::propagate_transform(app, schedule);
        app.add_systems(
            schedule,
            Self::transform_to_position_authoritative
                .in_set(PhysicsTransformSystems::TransformToPosition)
                .run_if(|config: Res<PhysicsTransformConfig>| config.transform_to_position),
        );
    }

    fn transform_to_position_authoritative(
        mut query: Query<
            (&GlobalTransform, &mut Position, &mut Rotation),
            (
                With<RigidBody>,
                Or<(Changed<Transform>, Changed<GlobalTransform>)>,
            ),
        >,
    ) {
        for (global_transform, mut position, mut rotation) in &mut query {
            // The pre-fixed PositionToTransform sync and propagation also trigger this query when
            // the user did not edit Transform. Only mutating Position/Rotation when the propagated
            // values actually differ avoids reporting a redundant physics-state change.
            let (_, global_rotation, global_translation) =
                global_transform.to_scale_rotation_translation();

            #[cfg(feature = "2d")]
            {
                let new_position = global_translation.truncate().adjust_precision();
                let new_rotation = Rotation::from(global_rotation.adjust_precision());
                if position.0 != new_position {
                    position.0 = new_position;
                }
                if *rotation != new_rotation {
                    *rotation = new_rotation;
                }
            }

            #[cfg(feature = "3d")]
            {
                let new_position = global_translation.adjust_precision();
                let new_rotation = Rotation(global_rotation.adjust_precision());
                if position.0 != new_position {
                    position.0 = new_position;
                }
                if *rotation != new_rotation {
                    *rotation = new_rotation;
                }
            }
        }
    }

    fn sync_position_to_transform(app: &mut App, schedule: impl ScheduleLabel) {
        let schedule = schedule.intern();

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

#[cfg(test)]
mod mode_tests {
    use super::*;
    use bevy_state::app::StatesPlugin;
    use bevy_transform::TransformPlugin;
    use lightyear_core::prelude::ConfirmedHistory;
    use lightyear_interpolation::prelude::{InterpolationMarkerPlugin, InterpolationRegistry};
    use lightyear_prediction::prelude::{PredictionMarkerPlugin, PredictionRegistry};
    use lightyear_replication::LightyearRepliconBackend;

    #[test]
    fn position_mode_defaults_to_sync_to_transform_disabled() {
        assert_eq!(
            AvianReplicationMode::default(),
            AvianReplicationMode::Position {
                sync_to_transform: false
            }
        );
    }

    #[test]
    fn interpolated_collider_root_keeps_position_to_transform_marker() {
        let mut app = App::new();
        app.add_plugins(LightyearAvianPlugin {
            update_syncs_manually: true,
            ..Default::default()
        });

        let root = app
            .world_mut()
            .spawn((
                Position::default(),
                Rotation::default(),
                Interpolated,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<ApplyPosToTransform>(root).is_some());

        // Avian uses a self-referential ColliderOf for a collider on the body root. This must not
        // make the root look like a compound child collider.
        app.world_mut()
            .entity_mut(root)
            .insert(ColliderOf { body: root });
        app.world_mut().flush();
        assert!(app.world().get::<ApplyPosToTransform>(root).is_some());

        // The same insertion order on a real child must remove the marker so its fixed local
        // Transform is preserved.
        let child = app
            .world_mut()
            .spawn((
                Position::default(),
                Rotation::default(),
                Interpolated,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().flush();
        assert!(app.world().get::<ApplyPosToTransform>(child).is_some());
        app.world_mut()
            .entity_mut(child)
            .insert(ColliderOf { body: root });
        app.world_mut().flush();
        assert!(app.world().get::<ApplyPosToTransform>(child).is_none());
    }

    #[test]
    fn interpolated_pose_propagates_to_fixed_offset_child_collider() {
        let mut app = App::new();
        app.add_plugins(TransformPlugin);
        app.add_plugins(LightyearAvianPlugin {
            register_physics_components: false,
            ..Default::default()
        });

        let root = app
            .world_mut()
            .spawn((
                Position::default(),
                Rotation::default(),
                Interpolated,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let child_local = Transform::from_xyz(0.75, 0.25, 0.0);
        let child = app
            .world_mut()
            .spawn((
                ChildOf(root),
                ColliderOf { body: root },
                Position::default(),
                Rotation::default(),
                Interpolated,
                child_local,
                GlobalTransform::default(),
            ))
            .id();

        app.update();

        // Delayed interpolation writes its sampled pose into Position/Rotation. Exercise the
        // downstream Avian sync and Bevy hierarchy propagation with an existing child collider.
        let sampled_position =
            Position(Vector::X * Scalar::from(3.0_f32) + Vector::Y * Scalar::from(2.0_f32));
        #[cfg(all(feature = "2d", not(feature = "3d")))]
        let sampled_rotation = Rotation::radians(Scalar::from(0.4_f32));
        #[cfg(all(feature = "3d", not(feature = "2d")))]
        let sampled_rotation = Rotation(Quaternion::from_rotation_z(Scalar::from(0.4_f32)));

        *app.world_mut().get_mut::<Position>(root).unwrap() = sampled_position;
        *app.world_mut().get_mut::<Rotation>(root).unwrap() = sampled_rotation;
        app.update();

        assert!(app.world().get::<ApplyPosToTransform>(root).is_some());
        assert!(app.world().get::<ApplyPosToTransform>(child).is_none());
        assert_eq!(*app.world().get::<Transform>(child).unwrap(), child_local);

        let root_transform = app.world().get::<Transform>(root).unwrap();
        #[cfg(all(feature = "2d", not(feature = "3d")))]
        let expected_root_translation = sampled_position.0.f32().extend(0.0);
        #[cfg(all(feature = "3d", not(feature = "2d")))]
        let expected_root_translation = sampled_position.0.f32();
        let expected_root_rotation = Quaternion::from(sampled_rotation).f32();
        assert!(
            root_transform
                .translation
                .distance(expected_root_translation)
                < 1e-5
        );
        assert!(
            root_transform
                .rotation
                .angle_between(expected_root_rotation)
                < 1e-5
        );

        let child_global = app
            .world()
            .get::<GlobalTransform>(child)
            .unwrap()
            .compute_transform();
        let expected_child_translation = root_transform.transform_point(child_local.translation);
        assert!(
            child_global
                .translation
                .distance(expected_child_translation)
                < 1e-5
        );
        assert!(
            (child_global.rotation * bevy_math::Vec3::X)
                .distance(root_transform.rotation * bevy_math::Vec3::X)
                < 1e-5
        );
    }

    #[test]
    fn position_mode_registers_pose_velocity_hermite_rule() {
        let mut app = App::new();
        app.add_plugins(LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            update_syncs_manually: true,
            ..Default::default()
        });

        let registry = app.world().resource::<InterpolationRegistry>();
        assert!(registry.interpolated::<Position>());
        assert!(registry.interpolated::<Rotation>());
        assert!(registry.interpolated::<LinearVelocity>());
        assert!(registry.interpolated::<AngularVelocity>());
        assert!(
            app.world()
                .components()
                .component_id::<ConfirmedHistory<Position>>()
                .is_some(),
            "the automatic rule must own delayed-interpolation history"
        );
    }

    #[test]
    fn position_mode_registers_the_default_physics_protocol() {
        let mut app = App::new();
        app.add_plugins(StatesPlugin);
        app.add_plugins(LightyearRepliconBackend);
        app.add_plugins((PredictionMarkerPlugin, InterpolationMarkerPlugin));
        app.init_resource::<PredictionRegistry>();
        app.add_plugins(LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            update_syncs_manually: true,
            ..Default::default()
        });

        let components = app.world().resource::<ComponentRegistry>();
        assert!(components.is_registered::<Position>());
        assert!(components.is_registered::<Rotation>());
        assert!(components.is_registered::<LinearVelocity>());
        assert!(components.is_registered::<AngularVelocity>());

        let prediction = app.world().resource::<PredictionRegistry>();
        assert!(!prediction.should_rollback(&Position::default(), &Position::default()));
        assert!(prediction.should_rollback(
            &Position::default(),
            &Position(Vector::X * Scalar::from(0.02_f32))
        ));
        #[cfg(all(feature = "2d", not(feature = "3d")))]
        {
            assert!(!prediction.should_rollback(
                &Rotation::default(),
                &Rotation::radians(Scalar::from(0.005_f32))
            ));
            assert!(prediction.should_rollback(
                &Rotation::default(),
                &Rotation::radians(Scalar::from(0.02_f32))
            ));
        }
        #[cfg(all(feature = "3d", not(feature = "2d")))]
        {
            assert!(!prediction.should_rollback(
                &Rotation::default(),
                &Rotation(Quaternion::from_rotation_x(Scalar::from(0.005_f32)))
            ));
            assert!(prediction.should_rollback(
                &Rotation::default(),
                &Rotation(Quaternion::from_rotation_x(Scalar::from(0.02_f32)))
            ));
        }
        assert!(!prediction.should_rollback(
            &LinearVelocity::default(),
            &LinearVelocity(Vector::X * Scalar::from(0.005_f32))
        ));
        assert!(prediction.should_rollback(
            &LinearVelocity::default(),
            &LinearVelocity(Vector::X * Scalar::from(0.02_f32))
        ));
        #[cfg(all(feature = "2d", not(feature = "3d")))]
        {
            assert!(!prediction.should_rollback(
                &AngularVelocity::default(),
                &AngularVelocity(Scalar::from(0.005_f32))
            ));
            assert!(prediction.should_rollback(
                &AngularVelocity::default(),
                &AngularVelocity(Scalar::from(0.02_f32))
            ));
        }
        #[cfg(all(feature = "3d", not(feature = "2d")))]
        {
            assert!(!prediction.should_rollback(
                &AngularVelocity::default(),
                &AngularVelocity(Vector::X * Scalar::from(0.005_f32))
            ));
            assert!(prediction.should_rollback(
                &AngularVelocity::default(),
                &AngularVelocity(Vector::X * Scalar::from(0.02_f32))
            ));
        }
    }

    #[test]
    fn custom_physics_protocol_uses_normal_rollback_builder() {
        fn never_rollback(_confirmed: &Position, _predicted: &Position) -> bool {
            false
        }

        let mut app = App::new();
        app.add_plugins(StatesPlugin);
        app.add_plugins(LightyearRepliconBackend);
        app.add_plugins((PredictionMarkerPlugin, InterpolationMarkerPlugin));
        app.init_resource::<PredictionRegistry>();
        app.add_plugins(LightyearAvianPlugin {
            register_physics_components: false,
            update_syncs_manually: true,
            ..Default::default()
        });
        app.component::<Position>()
            .replicate_filtered::<With<RigidBody>>()
            .predict()
            .with_rollback_condition(never_rollback);

        let prediction = app.world().resource::<PredictionRegistry>();
        assert!(!prediction.should_rollback(
            &Position::default(),
            &Position(Vector::X * Scalar::from(100.0_f32))
        ));
    }
}

#[cfg(all(test, feature = "2d", not(feature = "3d")))]
mod tests {
    use super::*;

    use bevy_ecs::system::RunSystemOnce;
    use bevy_time::{Fixed, Time};
    use core::time::Duration;
    use lightyear_frame_interpolation::{
        FrameInterpolate, FrameInterpolationHistory, FrameInterpolationPlugin,
    };

    fn seed_stationary_hermite_histories(app: &mut App, entity: Entity, include_previous: bool) {
        let mut entity = app.world_mut().entity_mut(entity);
        let mut rotation = entity
            .get_mut::<FrameInterpolationHistory<Rotation>>()
            .unwrap();
        rotation.current_value = Some(Rotation::default());
        rotation.previous_value = include_previous.then(Rotation::default);
        let mut linear = entity
            .get_mut::<FrameInterpolationHistory<LinearVelocity>>()
            .unwrap();
        linear.current_value = Some(LinearVelocity::default());
        linear.previous_value = include_previous.then(LinearVelocity::default);
        let mut angular = entity
            .get_mut::<FrameInterpolationHistory<AngularVelocity>>()
            .unwrap();
        angular.current_value = Some(AngularVelocity::default());
        angular.previous_value = include_previous.then(AngularVelocity::default);
    }

    fn stop_after_physics(mut velocities: Query<&mut LinearVelocity>) {
        for mut velocity in &mut velocities {
            *velocity = LinearVelocity::default();
        }
    }

    #[test]
    fn frame_history_captures_post_physics_application_changes() {
        let mut app = App::new();
        app.add_plugins((
            FrameInterpolationPlugin,
            LightyearAvianPlugin {
                update_syncs_manually: true,
                register_physics_components: false,
                ..Default::default()
            },
        ));
        app.add_systems(
            FixedPostUpdate,
            stop_after_physics
                .after(PhysicsSystems::Writeback)
                .before(PredictionSystems::UpdateHistory),
        );
        app.finish();

        let entity = app
            .world_mut()
            .spawn((
                Position::default(),
                Rotation::default(),
                LinearVelocity(Vector::X),
                AngularVelocity::default(),
                FrameInterpolate,
            ))
            .id();

        app.world_mut().run_schedule(FixedPostUpdate);

        assert_eq!(
            app.world()
                .get::<FrameInterpolationHistory<LinearVelocity>>(entity)
                .unwrap()
                .current_value,
            Some(LinearVelocity::default())
        );
    }

    #[test]
    fn position_mode_does_not_copy_stale_transform_into_physics() {
        let mut app = App::new();
        app.init_resource::<bevy_transform::systems::StaticTransformOptimizations>();
        app.add_plugins((
            PhysicsSchedulePlugin::default(),
            LightyearAvianPlugin {
                replication_mode: AvianReplicationMode::Position {
                    sync_to_transform: false,
                },
                ..Default::default()
            },
        ));
        app.finish();

        assert!(
            !app.world()
                .resource::<PhysicsTransformConfig>()
                .transform_to_position,
            "Position mode must keep physics authoritative without relying on frame restore"
        );

        let canonical_position = Position::from_xy(10.0, 20.0);
        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                canonical_position,
                Rotation::default(),
                Transform::from_xyz(-5.0, -6.0, 0.0),
                GlobalTransform::default(),
            ))
            .id();

        // Run twice so this also covers an existing entity whose Transform changes after the
        // synchronization systems have already observed it. No FrameInterpolationPlugin or
        // FrameInterpolate marker is installed in this app.
        app.world_mut().run_schedule(RunFixedMainLoop);
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Transform>()
            .unwrap()
            .translation = bevy_math::Vec3::new(-50.0, -60.0, 0.0);
        app.world_mut().run_schedule(RunFixedMainLoop);

        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&canonical_position)
        );
    }

    #[test]
    fn position_mode_writes_frame_interpolated_pose_to_transform() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.add_plugins((
            PhysicsSchedulePlugin::default(),
            FrameInterpolationPlugin,
            LightyearAvianPlugin {
                replication_mode: AvianReplicationMode::Position {
                    sync_to_transform: false,
                },
                ..Default::default()
            },
        ));
        app.interpolate_with::<Position>(InterpolationFns::no_history(|_, end, _| end));
        app.finish();

        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                Position::default(),
                Rotation::default(),
                Transform::default(),
                GlobalTransform::default(),
                FrameInterpolate,
            ))
            .id();

        // Warm up Avian's Changed<Position> filter so the next write must be detected from
        // frame interpolation rather than from the component's spawn tick.
        app.world_mut().run_schedule(PostUpdate);

        let visual_position = Position::from_xy(4.0, 6.0);
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<FrameInterpolationHistory<Position>>()
            .unwrap()
            .current_value = Some(visual_position);
        seed_stationary_hermite_histories(&mut app, entity, false);
        app.world_mut().run_schedule(PostUpdate);

        let transform = app.world().get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation.truncate(), visual_position.f32());
    }

    #[test]
    fn position_mode_can_import_transform_authored_during_fixed_update() {
        let mut app = App::new();
        app.init_resource::<bevy_transform::systems::StaticTransformOptimizations>();
        app.init_resource::<Time>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(500));
        app.add_plugins((
            PhysicsSchedulePlugin::default(),
            FrameInterpolationPlugin,
            LightyearAvianPlugin {
                replication_mode: AvianReplicationMode::Position {
                    sync_to_transform: true,
                },
                ..Default::default()
            },
        ));
        app.interpolate_with::<Position>(InterpolationFns::no_history(|start, end, t| {
            Position(start.0.lerp(end.0, t as Scalar))
        }));
        app.finish();

        assert!(
            app.world()
                .resource::<PhysicsTransformConfig>()
                .transform_to_position
        );

        let canonical_position = Position::from_xy(10.0, 20.0);
        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                canonical_position,
                Rotation::default(),
                Transform::from_xyz(10.0, 20.0, 0.0),
                GlobalTransform::default(),
                FrameInterpolate,
                FrameInterpolationHistory::<Position> {
                    previous_value: Some(Position::default()),
                    current_value: Some(canonical_position),
                },
            ))
            .id();

        seed_stationary_hermite_histories(&mut app, entity, true);

        // PostUpdate leaves the interpolated visual pose in both Position and Transform.
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&Position::from_xy(5.0, 10.0))
        );

        // Before FixedUpdate, restore the canonical physics pose and use it to canonicalize
        // Transform. This prevents the visual pose from being imported into physics.
        app.world_mut().run_schedule(RunFixedMainLoop);
        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&canonical_position)
        );
        assert_eq!(
            app.world()
                .get::<Transform>(entity)
                .unwrap()
                .translation
                .truncate(),
            canonical_position.f32()
        );

        // Model a gameplay system authoring Transform during FixedUpdate. The fixed post-update
        // prepare pass must import it before Avian's physics step.
        let authored_position = Position::from_xy(30.0, 40.0);
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Transform>()
            .unwrap()
            .translation = authored_position.f32().extend(0.0);
        app.world_mut().run_schedule(FixedPostUpdate);

        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&authored_position)
        );
    }

    #[test]
    fn child_position_tracks_local_transform_and_reparenting() {
        let mut app = App::new();
        let body1_position = Position(Vector::new(10.0, 0.0));
        let body1_rotation = Rotation::radians(core::f32::consts::FRAC_PI_2);
        let body1 = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                body1_position,
                body1_rotation,
                GlobalTransform::IDENTITY,
            ))
            .id();
        let body2_position = Position(Vector::new(-5.0, 3.0));
        let body2_rotation = Rotation::radians(-0.5);
        let body2 = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                body2_position,
                body2_rotation,
                GlobalTransform::IDENTITY,
            ))
            .id();
        let local1 = ColliderTransform {
            translation: Vector::new(2.0, 0.0),
            rotation: Rotation::radians(0.25),
            scale: Vector::ONE,
        };
        let child = app
            .world_mut()
            .spawn((
                ChildOf(body1),
                GlobalTransform::IDENTITY,
                Position::default(),
                Rotation::default(),
                ColliderOf { body: body1 },
            ))
            .id();
        app.world_mut().entity_mut(child).insert(local1);

        app.world_mut()
            .run_system_once(LightyearAvianPlugin::update_child_collider_position)
            .unwrap();

        assert_eq!(
            *app.world().get::<Position>(child).unwrap(),
            Position(body1_position.0 + body1_rotation * local1.translation)
        );
        assert_eq!(
            *app.world().get::<Rotation>(child).unwrap(),
            body1_rotation * local1.rotation
        );

        let local2 = ColliderTransform {
            translation: Vector::new(-1.0, 4.0),
            rotation: Rotation::radians(-0.3),
            scale: Vector::ONE,
        };
        app.world_mut()
            .entity_mut(child)
            .insert((ChildOf(body2), ColliderOf { body: body2 }));
        app.world_mut().entity_mut(child).insert(local2);
        app.world_mut()
            .run_system_once(LightyearAvianPlugin::update_child_collider_position)
            .unwrap();

        assert_eq!(
            *app.world().get::<Position>(child).unwrap(),
            Position(body2_position.0 + body2_rotation * local2.translation)
        );
        assert_eq!(
            *app.world().get::<Rotation>(child).unwrap(),
            body2_rotation * local2.rotation
        );
    }
}

#[cfg(all(test, feature = "3d", not(feature = "2d")))]
mod tests_3d {
    use super::*;

    use bevy_time::{Fixed, Time};
    use core::time::Duration;
    use lightyear_frame_interpolation::{
        FrameInterpolate, FrameInterpolationHistory, FrameInterpolationPlugin,
    };

    fn seed_stationary_hermite_histories(app: &mut App, entity: Entity, include_previous: bool) {
        let mut entity = app.world_mut().entity_mut(entity);
        let mut rotation = entity
            .get_mut::<FrameInterpolationHistory<Rotation>>()
            .unwrap();
        rotation.current_value = Some(Rotation::default());
        rotation.previous_value = include_previous.then(Rotation::default);
        let mut linear = entity
            .get_mut::<FrameInterpolationHistory<LinearVelocity>>()
            .unwrap();
        linear.current_value = Some(LinearVelocity::default());
        linear.previous_value = include_previous.then(LinearVelocity::default);
        let mut angular = entity
            .get_mut::<FrameInterpolationHistory<AngularVelocity>>()
            .unwrap();
        angular.current_value = Some(AngularVelocity::default());
        angular.previous_value = include_previous.then(AngularVelocity::default);
    }

    #[test]
    fn position_mode_does_not_copy_stale_transform_into_physics() {
        let mut app = App::new();
        app.init_resource::<bevy_transform::systems::StaticTransformOptimizations>();
        app.add_plugins((
            PhysicsSchedulePlugin::default(),
            LightyearAvianPlugin {
                replication_mode: AvianReplicationMode::Position {
                    sync_to_transform: false,
                },
                ..Default::default()
            },
        ));
        app.finish();

        assert!(
            !app.world()
                .resource::<PhysicsTransformConfig>()
                .transform_to_position,
            "Position mode must keep physics authoritative without relying on frame restore"
        );

        let canonical_position = Position::new(Vector::new(10.0, 20.0, 30.0));
        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                canonical_position,
                Rotation::default(),
                Transform::from_xyz(-5.0, -6.0, -7.0),
                GlobalTransform::default(),
            ))
            .id();

        app.world_mut().run_schedule(RunFixedMainLoop);
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Transform>()
            .unwrap()
            .translation = bevy_math::Vec3::new(-50.0, -60.0, -70.0);
        app.world_mut().run_schedule(RunFixedMainLoop);

        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&canonical_position)
        );
    }

    #[test]
    fn position_mode_writes_frame_interpolated_pose_to_transform() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.add_plugins((
            PhysicsSchedulePlugin::default(),
            FrameInterpolationPlugin,
            LightyearAvianPlugin {
                replication_mode: AvianReplicationMode::Position {
                    sync_to_transform: false,
                },
                ..Default::default()
            },
        ));
        app.interpolate_with::<Position>(InterpolationFns::no_history(|_, end, _| end));
        app.finish();

        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                Position::default(),
                Rotation::default(),
                Transform::default(),
                GlobalTransform::default(),
                FrameInterpolate,
            ))
            .id();

        // Warm up Avian's Changed<Position> filter so the next write must be detected from
        // frame interpolation rather than from the component's spawn tick.
        app.world_mut().run_schedule(PostUpdate);

        let visual_position = Position::new(Vector::new(4.0, 6.0, 8.0));
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<FrameInterpolationHistory<Position>>()
            .unwrap()
            .current_value = Some(visual_position);
        seed_stationary_hermite_histories(&mut app, entity, false);
        app.world_mut().run_schedule(PostUpdate);

        let transform = app.world().get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation, visual_position.f32());
    }

    #[test]
    fn position_mode_can_import_transform_authored_during_fixed_update() {
        let mut app = App::new();
        app.init_resource::<bevy_transform::systems::StaticTransformOptimizations>();
        app.init_resource::<Time>();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(500));
        app.add_plugins((
            PhysicsSchedulePlugin::default(),
            FrameInterpolationPlugin,
            LightyearAvianPlugin {
                replication_mode: AvianReplicationMode::Position {
                    sync_to_transform: true,
                },
                ..Default::default()
            },
        ));
        app.interpolate_with::<Position>(InterpolationFns::no_history(|start, end, t| {
            Position(start.0.lerp(end.0, t as Scalar))
        }));
        app.finish();

        assert!(
            app.world()
                .resource::<PhysicsTransformConfig>()
                .transform_to_position
        );

        let canonical_position = Position::new(Vector::new(10.0, 20.0, 30.0));
        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                canonical_position,
                Rotation::default(),
                Transform::from_xyz(10.0, 20.0, 30.0),
                GlobalTransform::default(),
                FrameInterpolate,
                FrameInterpolationHistory::<Position> {
                    previous_value: Some(Position::default()),
                    current_value: Some(canonical_position),
                },
            ))
            .id();

        seed_stationary_hermite_histories(&mut app, entity, true);

        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&Position::new(Vector::new(5.0, 10.0, 15.0)))
        );

        app.world_mut().run_schedule(RunFixedMainLoop);
        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&canonical_position)
        );
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            canonical_position.f32()
        );

        let authored_position = Position::new(Vector::new(30.0, 40.0, 50.0));
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Transform>()
            .unwrap()
            .translation = authored_position.f32();
        app.world_mut().run_schedule(FixedPostUpdate);

        assert_eq!(
            app.world().get::<Position>(entity),
            Some(&authored_position)
        );
    }
}
