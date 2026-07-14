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
!*/
use alloc::vec::Vec;
#[cfg(all(feature = "2d", not(feature = "3d")))]
use avian2d::{
    collider_tree::{
        ColliderTreeProxyFlags, ColliderTreeProxyKey, ColliderTreeType, ColliderTrees, MovedProxies,
    },
    collision::collider::{ColliderAabb, EnlargedAabb},
    collision::contact_types::ContactEdgeFlags,
    dynamics::solver::{
        constraint_graph::ConstraintGraph,
        islands::{BodyIslandNode, PhysicsIslands},
        joint_graph::JointGraph,
    },
    math::*,
    physics_transform::*,
    prelude::*,
};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use avian3d::{
    collider_tree::{
        ColliderTreeProxyFlags, ColliderTreeProxyKey, ColliderTreeType, ColliderTrees, MovedProxies,
    },
    collision::collider::{ColliderAabb, EnlargedAabb},
    collision::contact_types::ContactEdgeFlags,
    dynamics::solver::{
        constraint_graph::ConstraintGraph,
        islands::{BodyIslandNode, PhysicsIslands},
        joint_graph::JointGraph,
    },
    math::*,
    physics_transform::*,
    prelude::*,
};
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

use lightyear_core::timeline::is_in_rollback;
use lightyear_frame_interpolation::FrameInterpolationSystems;
use lightyear_interpolation::prelude::{AppInterpolationExt, InterpolationFns};
use lightyear_prediction::plugin::PredictionSystems;
use lightyear_prediction::prelude::{
    PredictionAppRegistrationExt, PredictionBuilderExt, PredictionManager, RollbackSystems,
};
use lightyear_replication::prelude::{AppComponentExt, TransformLinearInterpolation};

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
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LightyearAvianPlugin {
    /// The replication mode to use for the Avian plugin.
    pub replication_mode: AvianReplicationMode,
    /// If true, Lightyear does not install automatic `Position`/`Rotation`/`Transform` sync systems.
    ///
    /// Enable this if you are an advanced user and want to handle all synchronization manually.
    pub update_syncs_manually: bool,
    /// If True, the plugin will rollback resources that are not replicated, such as Collisions.
    /// Enable this if you are using deterministic replication (i.e. are not replicating state).
    ///
    /// If Avian's `IslandPlugin` is enabled, island rollback state is registered automatically
    /// during `finish()`. If `IslandSleepingPlugin` is also enabled, sleeping state is rolled back too.
    pub rollback_resources: bool,
}

#[derive(Resource, Clone, Debug, Default)]
struct RollbackMovedProxies {
    // Avian's `MovedProxies` resource is not `Clone`; keep a cloneable snapshot
    // so rollback replay uses the same broad-phase update set as the first run.
    proxies: Vec<ColliderTreeProxyKey>,
}

#[derive(Clone, Copy)]
struct RollbackColliderProxy {
    proxy_key: ColliderTreeProxyKey,
    collider: Entity,
    body: Option<Entity>,
    aabb: ColliderAabb,
    layers: CollisionLayers,
    flags: ColliderTreeProxyFlags,
}

impl Plugin for LightyearAvianPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsTransformConfig>();
        match self.replication_mode {
            AvianReplicationMode::Position { sync_to_transform } => {
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
            app.resource::<ContactGraph>().local_rollback();
            app.resource::<ConstraintGraph>().local_rollback();
            app.resource::<RollbackMovedProxies>().local_rollback();
            app.resource::<PhysicsIslands>().local_rollback();
            app.init_resource::<ContactGraph>();
            app.init_resource::<ConstraintGraph>();
            app.local_rollback::<CollidingEntities>();
            // `ColliderTrees` cannot be cloned for rollback, but its leaf AABBs
            // are derived from these cloneable collider components.
            app.local_rollback::<ColliderAabb>();
            app.local_rollback::<EnlargedAabb>();
            app.init_resource::<RollbackMovedProxies>();
            app.add_systems(
                FixedPostUpdate,
                Self::record_moved_proxies_for_rollback
                    .after(PhysicsSystems::StepSimulation)
                    .before(PredictionSystems::UpdateHistory),
            );
            app.add_systems(
                PreUpdate,
                Self::restore_collider_tree_from_enlarged_aabbs
                    .after(RollbackSystems::Prepare)
                    .before(RollbackSystems::Rollback)
                    .run_if(is_in_rollback),
            );
        }
    }

    fn finish(&self, app: &mut App) {
        if self.rollback_resources && app.is_plugin_added::<IslandPlugin>() {
            let rollback_sleeping = app.is_plugin_added::<IslandSleepingPlugin>();
            Self::add_island_rollback(app, rollback_sleeping);
        }
    }
}

impl LightyearAvianPlugin {
    // Add a low-priority interpolation function for Transform
    // (that could be used for Correction or FrameInterpolation)
    fn add_transform_frame_interpolation_rule(app: &mut App) {
        app.interpolate_with_priority::<Transform>(
            0,
            InterpolationFns::no_history(TransformLinearInterpolation::lerp),
        );
    }

    fn add_island_rollback(app: &mut App, rollback_sleeping: bool) {
        app.local_rollback::<BodyIslandNode>();
        if rollback_sleeping {
            app.local_rollback::<Sleeping>();
            app.local_rollback::<SleepTimer>();
        }
    }

    fn record_moved_proxies_for_rollback(
        moved_proxies: Res<MovedProxies>,
        mut rollback_moved_proxies: ResMut<RollbackMovedProxies>,
    ) {
        rollback_moved_proxies.proxies.clear();
        rollback_moved_proxies
            .proxies
            .extend_from_slice(moved_proxies.proxies());
    }

    fn restore_collider_tree_from_enlarged_aabbs(
        prediction_manager: Single<&PredictionManager, With<lightyear_core::timeline::Rollback>>,
        mut trees: ResMut<ColliderTrees>,
        mut moved_proxies: ResMut<MovedProxies>,
        rollback_moved_proxies: Res<RollbackMovedProxies>,
        mut contact_graph: ResMut<ContactGraph>,
        joint_graph: Option<Res<JointGraph>>,
        colliders: Query<(&ColliderTreeProxyKey, &EnlargedAabb), Without<ColliderDisabled>>,
    ) {
        if prediction_manager.get_rollback_start_tick().is_none() {
            return;
        }
        // The rollback just restored `EnlargedAabb`; rebuild Avian's tree
        // leaves from that state before replaying physics. A stale tree can
        // miss contacts even when Position/Velocity were rolled back correctly.
        moved_proxies.clear();
        for tree in trees.iter_trees_mut() {
            tree.moved_proxies.clear();
        }

        for (proxy_key, enlarged_aabb) in &colliders {
            if *proxy_key == ColliderTreeProxyKey::PLACEHOLDER {
                continue;
            }
            let tree = trees.tree_for_type_mut(proxy_key.tree_type());
            if tree.get_proxy(proxy_key.id()).is_none() {
                continue;
            }
            tree.set_proxy_aabb(proxy_key.id(), enlarged_aabb.get().into());
        }

        for tree in trees.iter_trees_mut() {
            tree.refit_all();
        }

        Self::repair_missing_contact_pairs_from_restored_aabbs(
            &trees,
            &colliders,
            &mut contact_graph,
            joint_graph.as_deref(),
        );

        // Preserve the original moved-proxy set instead of marking every proxy
        // moved; extra pairs can perturb contact ordering and produce tiny
        // floating point differences.
        for proxy_key in rollback_moved_proxies.proxies.iter().copied() {
            if proxy_key == ColliderTreeProxyKey::PLACEHOLDER {
                continue;
            }
            let tree = trees.tree_for_type_mut(proxy_key.tree_type());
            if tree.get_proxy(proxy_key.id()).is_some() && moved_proxies.insert(proxy_key) {
                tree.moved_proxies.push(proxy_key.id());
            }
        }
    }

    fn repair_missing_contact_pairs_from_restored_aabbs(
        trees: &ColliderTrees,
        colliders: &Query<(&ColliderTreeProxyKey, &EnlargedAabb), Without<ColliderDisabled>>,
        contact_graph: &mut ContactGraph,
        joint_graph: Option<&JointGraph>,
    ) {
        // `ColliderTrees` is not cloneable, and a stale or incomplete tree can
        // miss contacts during replay. Preserve restored graph state and only
        // repair pairs that should exist according to the restored AABBs.

        let mut proxies = Vec::new();
        for (proxy_key, enlarged_aabb) in colliders {
            if *proxy_key == ColliderTreeProxyKey::PLACEHOLDER {
                continue;
            }
            let Some(proxy) = trees.get_proxy(*proxy_key) else {
                continue;
            };
            proxies.push(RollbackColliderProxy {
                proxy_key: *proxy_key,
                collider: proxy.collider,
                body: proxy.body,
                aabb: enlarged_aabb.get(),
                layers: proxy.layers,
                flags: proxy.flags,
            });
        }

        proxies.sort_by_key(|proxy| (proxy.proxy_key.tree_type() as u8, proxy.proxy_key.id().id()));

        let mut pairs = Vec::new();
        for (index, proxy1) in proxies.iter().enumerate() {
            for proxy2 in &proxies[index + 1..] {
                if !proxy1.aabb.intersects(&proxy2.aabb) {
                    continue;
                }
                if !proxy1.layers.interacts_with(proxy2.layers) {
                    continue;
                }
                if proxy1.body == proxy2.body {
                    continue;
                }
                let flags_union = proxy1.flags.union(proxy2.flags);
                if proxy1.proxy_key.tree_type() == ColliderTreeType::Static
                    && proxy2.proxy_key.tree_type() == ColliderTreeType::Static
                    && !flags_union.contains(ColliderTreeProxyFlags::SENSOR)
                {
                    continue;
                }
                if let (Some(joint_graph), Some(body1), Some(body2)) =
                    (joint_graph, proxy1.body, proxy2.body)
                    && joint_graph
                        .joints_between(body1, body2)
                        .any(|edge| edge.collision_disabled)
                {
                    continue;
                }
                pairs.push((*proxy1, *proxy2, flags_union));
            }
        }

        let mut repaired_pairs = 0;
        let mut skipped_custom_filter_pairs = 0;
        for (proxy1, proxy2, flags_union) in pairs {
            if contact_graph.contains(proxy1.collider, proxy2.collider) {
                continue;
            }
            if flags_union.contains(ColliderTreeProxyFlags::CUSTOM_FILTER) {
                skipped_custom_filter_pairs += 1;
                continue;
            }

            let mut contact_edge = ContactEdge::new(proxy1.collider, proxy2.collider);
            contact_edge.body1 = proxy1.body;
            contact_edge.body2 = proxy2.body;
            contact_edge.flags.set(
                ContactEdgeFlags::CONTACT_EVENTS,
                flags_union.contains(ColliderTreeProxyFlags::CONTACT_EVENTS),
            );

            contact_graph.add_edge_with(contact_edge, |contact_pair| {
                contact_pair.body1 = proxy1.body;
                contact_pair.body2 = proxy2.body;
                contact_pair.flags.set(
                    ContactPairFlags::MODIFY_CONTACTS,
                    flags_union.contains(ColliderTreeProxyFlags::MODIFY_CONTACTS),
                );
                contact_pair.flags.set(
                    ContactPairFlags::GENERATE_CONSTRAINTS,
                    !flags_union.contains(ColliderTreeProxyFlags::BODY_DISABLED)
                        && !flags_union.contains(ColliderTreeProxyFlags::SENSOR),
                );
            });
            repaired_pairs += 1;
        }

        if repaired_pairs > 0 || skipped_custom_filter_pairs > 0 {
            trace!(
                repaired_pairs,
                skipped_custom_filter_pairs,
                "Repaired Avian ContactGraph from restored rollback AABBs"
            );
        }
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
        if app
            .world()
            .resource::<PhysicsTransformConfig>()
            .position_to_transform
        {
            // Make sure that PositionToTransform sync also runs for Interpolated entities
            app.try_register_required_components::<Position, ApplyPosToTransform>()
                .ok();
            app.try_register_required_components::<Rotation, ApplyPosToTransform>()
                .ok();

            // NOTE: we do NOT register Transform as required for Position/Rotation because
            //  they might not be added at the same time (e.g. on Interpolated entities).
            //  The `add_transform` system below handles adding Transform when both are present.
            //  For physics entities, Transform is registered as required for Collider above.
        }
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
    use super::AvianReplicationMode;

    #[test]
    fn position_mode_defaults_to_sync_to_transform_disabled() {
        assert_eq!(
            AvianReplicationMode::default(),
            AvianReplicationMode::Position {
                sync_to_transform: false
            }
        );
    }
}

#[cfg(all(test, feature = "2d", not(feature = "3d")))]
mod tests {
    use super::*;

    use avian2d::collider_tree::ColliderTreeProxy;
    use bevy_ecs::system::RunSystemOnce;
    use bevy_time::{Fixed, Time};
    use core::time::Duration;
    use lightyear_frame_interpolation::{
        FrameInterpolate, FrameInterpolationHistory, FrameInterpolationPlugin,
    };

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

    fn add_dynamic_proxy(app: &mut App, collider: Entity, body: Entity, aabb: ColliderAabb) {
        let proxy_id = app
            .world_mut()
            .resource_mut::<ColliderTrees>()
            .dynamic_tree
            .add_proxy(
                aabb.into(),
                ColliderTreeProxy {
                    collider,
                    body: Some(body),
                    layers: CollisionLayers::default(),
                    flags: ColliderTreeProxyFlags::empty(),
                },
            );
        let proxy_key = ColliderTreeProxyKey::new(proxy_id, ColliderTreeType::Dynamic);
        app.world_mut()
            .entity_mut(collider)
            .insert((proxy_key, EnlargedAabb::new(aabb)));
    }

    fn repair_contact_graph_system(
        trees: Res<ColliderTrees>,
        mut contact_graph: ResMut<ContactGraph>,
        colliders: Query<(&ColliderTreeProxyKey, &EnlargedAabb), Without<ColliderDisabled>>,
    ) {
        LightyearAvianPlugin::repair_missing_contact_pairs_from_restored_aabbs(
            &trees,
            &colliders,
            &mut contact_graph,
            None,
        );
    }

    #[test]
    fn repairs_missing_contact_pair_from_restored_aabbs() {
        let mut app = App::new();
        app.init_resource::<ColliderTrees>();
        app.init_resource::<ContactGraph>();

        let body1 = app.world_mut().spawn_empty().id();
        let body2 = app.world_mut().spawn_empty().id();
        let collider1 = app.world_mut().spawn_empty().id();
        let collider2 = app.world_mut().spawn_empty().id();

        add_dynamic_proxy(
            &mut app,
            collider1,
            body1,
            ColliderAabb::new(Vector::ZERO, Vector::splat(1.0)),
        );
        add_dynamic_proxy(
            &mut app,
            collider2,
            body2,
            ColliderAabb::new(Vector::new(1.5, 0.0), Vector::splat(1.0)),
        );

        app.world_mut()
            .run_system_once(repair_contact_graph_system)
            .unwrap();

        assert!(
            app.world()
                .resource::<ContactGraph>()
                .contains(collider1, collider2)
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
