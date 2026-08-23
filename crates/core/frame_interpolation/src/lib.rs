//! This module is not related to the interpolating between server updates.
//! Instead, it is responsible for interpolating between FixedUpdate ticks during the Update state.
//!
//! Usually, the simulation is run during the FixedUpdate schedule so that it doesn't depend on frame rate.
//! This can cause some visual inconsistencies (jitter) because the frames (Update schedule) don't correspond exactly to
//! the FixedUpdate schedule: there can be frames with several fixed-update ticks, and some frames with no fixed-update ticks.
//!
//! To solve this, we visually display the state of the game with 1 tick of delay.
//! For example if on the Update state we have an overstep of 0.7 and the current tick is 10,
//! we display the state of the game interpolated at 0.7 between tick 9 and tick 10.
//!
//! Another way to solve this would be to run an extra 'partial' simulation step with 0.7 dt and use this for the visual state.
//!
//! To enable FrameInterpolation:
//! - register an interpolation rule for the component or bundle. Existing
//!   interpolation rules are reused by default; if the component should not use
//!   delayed interpolation history, register it with
//!   [`InterpolationFns::no_history`](lightyear_interpolation::rules::InterpolationFns::no_history).
//! - FrameInterpolation is not enabled by default; add [`FrameInterpolationPlugin`] manually
//! - To enable FrameInterpolation on a given entity, add the type-erased [`FrameInterpolate`] marker to it manually
//!
//! Frame interpolation writes through Bevy's change-detection-aware component
//! access. Systems ordered after [`FrameInterpolationSystems::Interpolate`] can
//! therefore use `Changed<C>` to react to the visual value.
//! ```rust,ignore
//! # use lightyear_frame_interpolation::prelude::*;
//! # use lightyear_interpolation::prelude::*;
//! # use bevy_app::App;
//! # use bevy_ecs::prelude::*;
//!
//! # #[derive(Component, PartialEq, Clone, Debug)]
//! # struct Component1(f32);
//! # fn lerp_component(start: Component1, end: Component1, t: f32) -> Component1 {
//! #     Component1(start.0 + (end.0 - start.0) * t)
//! # }
//!
//! let mut app = App::new();
//! app.add_plugins(FrameInterpolationPlugin);
//! app.interpolate_with::<Component1>(InterpolationFns::no_history(lerp_component));
//!
//! fn spawn_entity(mut commands: Commands) {
//!     commands.spawn(FrameInterpolate);
//! }
//! ```

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod archetypes;

use crate::archetypes::FrameInterpolationWorld;
use bevy_app::prelude::*;
use bevy_ecs::{prelude::*, schedule::common_conditions::not};
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time};
pub use lightyear_core::frame_interpolation::{FrameInterpolate, FrameInterpolationHistory};
use lightyear_core::timeline::is_in_rollback;
use lightyear_interpolation::registry::InterpolationRegistry;
use lightyear_interpolation::rules::frame_interpolate::FrameInterpolationContext;
use lightyear_replication::ReplicationSystems;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use serde::{Deserialize, Serialize};

/// System sets used by [`FrameInterpolationPlugin`].
///
/// These sets help order the systems responsible for:
/// - Restoring the actual component values before other game logic.
/// - Updating the history of component values used for interpolation.
/// - Performing the visual interpolation itself.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum FrameInterpolationSystems {
    /// Restore the correct component values when we start the FixedMainLoop.
    Restore,
    /// Update the previous/current component values used for visual interpolation.
    Update,
    /// Interpolate the visual state of the game with 1 tick of delay.
    Interpolate,
}

#[deprecated(note = "Use FrameInterpolationSystems instead")]
pub type FrameInterpolationSet = FrameInterpolationSystems;

/// If present, this marker indicates that we will skip applying frame interpolation.
///
/// This can be useful for example if a character teleports and you don't want
/// to interpolate between the two positions.
///
/// You can add this directly on the client-side, or you can also add it on the
/// sender-side and replicate the component.
#[derive(Component, PartialEq, Serialize, Deserialize, Clone, Debug, Reflect)]
pub struct SkipFrameInterpolation;

/// Linear frame interpolation for components implementing [`Ease`].
pub fn linear_frame_interpolation<C: Ease + Clone>(start: C, end: C, t: f32) -> C {
    let curve = EasingCurve::new(start, end, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

/// Bevy plugin that enables type-erased frame interpolation.
///
/// This plugin adds systems to store fixed-update component values and then
/// interpolate between the previous and current fixed tick during `PostUpdate`,
/// using [`Time<Fixed>`]'s overstep. This helps smooth visuals when the
/// rendering framerate doesn't align perfectly with the fixed simulation rate.
///
/// To use this, register an interpolation rule in the [`InterpolationRegistry`]
/// and add [`FrameInterpolate`] to entities for which you want visual
/// interpolation.
///
/// # Examples
///
/// ```rust,ignore
/// use bevy_app::App;
/// use bevy_ecs::prelude::*;
/// use lightyear_frame_interpolation::prelude::*;
/// use lightyear_interpolation::prelude::*;
///
/// #[derive(Component, Clone, PartialEq)]
/// struct Position(f32);
///
/// fn lerp_position(start: Position, end: Position, t: f32) -> Position {
///     Position(start.0 + (end.0 - start.0) * t)
/// }
///
/// let mut app = App::new();
/// app.add_plugins(FrameInterpolationPlugin);
/// app.interpolate_with::<Position>(InterpolationFns::no_history(lerp_position));
/// ```
#[derive(Default)]
pub struct FrameInterpolationPlugin;

impl Plugin for FrameInterpolationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InterpolationRegistry>();
        // SETS
        app.configure_sets(
            RunFixedMainLoop,
            FrameInterpolationSystems::Restore.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
        );
        // We don't update frame interpolation history during rollback because
        // that would be a waste to do for each rollback frame.
        // At the end of rollback, lightyear_prediction manually updates the
        // FrameInterpolationHistory component.
        app.configure_sets(
            FixedPostUpdate,
            FrameInterpolationSystems::Update.run_if(not(is_in_rollback)),
        );
        app.configure_sets(
            PostUpdate,
            FrameInterpolationSystems::Interpolate
                .before(bevy_transform::TransformSystems::Propagate)
                // We don't want the visual interpolation value to be the one replicated.
                .after(ReplicationSystems::Send),
        );

        // SYSTEMS
        app.add_systems(
            RunFixedMainLoop,
            restore_frame_interpolation.in_set(FrameInterpolationSystems::Restore),
        );
        app.add_systems(
            FixedPostUpdate,
            update_frame_interpolation_histories.in_set(FrameInterpolationSystems::Update),
        );
        app.add_systems(
            PostUpdate,
            apply_frame_interpolation.in_set(FrameInterpolationSystems::Interpolate),
        );
    }

    fn finish(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .finalize();
    }
}

pub(crate) fn restore_frame_interpolation(
    mut frame_world: FrameInterpolationWorld,
    interpolation_registry: Res<InterpolationRegistry>,
) {
    frame_world.update_archetypes(&interpolation_registry);
    let world = frame_world.world;
    for (archetype, cached_archetype) in frame_world.iter_archetypes() {
        for component in &cached_archetype.history_components {
            (component.restore_frame_history())(world, archetype, component);
        }
    }
}

pub(crate) fn update_frame_interpolation_histories(
    mut frame_world: FrameInterpolationWorld,
    interpolation_registry: Res<InterpolationRegistry>,
    mut commands: Commands,
) {
    let mut deferred_apply = DeferredEntityCommands::default();

    frame_world.update_archetypes(&interpolation_registry);
    let world = frame_world.world;
    for (archetype, cached_archetype) in frame_world.iter_archetypes() {
        for component in &cached_archetype.history_components {
            (component.update_frame_history())(world, archetype, component, &mut deferred_apply);
        }
    }

    deferred_apply.apply(&mut commands);
}

pub(crate) fn apply_frame_interpolation(
    time: Res<Time<Fixed>>,
    mut frame_world: FrameInterpolationWorld,
    interpolation_registry: Res<InterpolationRegistry>,
) {
    let ctx = FrameInterpolationContext {
        overstep: time.overstep_fraction().clamp(0.0, 1.0),
        sample_delta_secs: Some(time.timestep().as_secs_f32()),
    };

    frame_world.update_archetypes(&interpolation_registry);
    let world = frame_world.world;
    for (archetype, cached_archetype) in frame_world.iter_archetypes() {
        for component in &cached_archetype.apply_callbacks {
            (component.apply_frame_interpolation())(
                world,
                archetype,
                &interpolation_registry,
                component.rule_id(),
                ctx,
                cached_archetype.skip_interpolation,
            );
        }
    }
}

/// Common frame interpolation exports.
pub mod prelude {
    #[allow(deprecated)]
    pub use crate::{
        FrameInterpolate, FrameInterpolationHistory, FrameInterpolationPlugin,
        FrameInterpolationSet, FrameInterpolationSystems, SkipFrameInterpolation,
        linear_frame_interpolation,
    };
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use bevy_app::{App, FixedPostUpdate, PostUpdate, RunFixedMainLoop};
    use bevy_replicon::prelude::RepliconSharedPlugin;
    use bevy_state::app::StatesPlugin;
    use core::time::Duration;
    use lightyear_core::prelude::ConfirmedHistory;
    use lightyear_interpolation::plugin::InterpolationMarkerPlugin;
    use lightyear_interpolation::registry::AppInterpolationExt;
    use lightyear_interpolation::rules::{
        InterpolationFns, InterpolationFnsExt, InterpolationSampleContext,
    };
    use lightyear_replication::prelude::AppComponentExt;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct FrameA(f32);

    #[derive(Component, Clone, Debug, PartialEq)]
    struct FrameB(f32);

    #[derive(Component)]
    struct DisabledFrameHistory;

    #[derive(Component, Clone, Debug, PartialEq)]
    #[component(storage = "SparseSet")]
    struct SparseFrame(f32);

    #[derive(Resource, Default)]
    struct ObservedFrameInterpolationChanges {
        table: bool,
        sparse_set: bool,
    }

    fn observe_frame_interpolation_changes(
        table: Query<(), Changed<FrameA>>,
        sparse_set: Query<(), Changed<SparseFrame>>,
        mut observed: ResMut<ObservedFrameInterpolationChanges>,
    ) {
        observed.table = !table.is_empty();
        observed.sparse_set = !sparse_set.is_empty();
    }

    fn bundle_lerp(start: (FrameA, FrameB), end: (FrameA, FrameB), t: f32) -> (FrameA, FrameB) {
        (
            FrameA(100.0 + start.0.0 + (end.0.0 - start.0.0) * t),
            FrameB(200.0 + start.1.0 + (end.1.0 - start.1.0) * t),
        )
    }

    fn bundle_context_lerp(
        _start: (FrameA, FrameB),
        _end: (FrameA, FrameB),
        context: InterpolationSampleContext,
    ) -> (FrameA, FrameB) {
        (
            FrameA(context.t),
            FrameB(context.sample_delta_secs.unwrap_or_default()),
        )
    }

    /// Checks that rule registration is rejected after the frame plugin finalizes the registry.
    #[test]
    #[should_panic(
        expected = "cannot register interpolation rules after InterpolationRegistry has been finalized"
    )]
    fn frame_plugin_rejects_late_rule_registration() {
        let mut app = App::new();
        app.add_plugins(FrameInterpolationPlugin);
        app.finish();

        app.interpolate_with::<FrameA>(InterpolationFns::disabled());
    }

    /// Checks that adding `FrameInterpolate` later initializes existing component history.
    #[test]
    fn frame_update_inserts_history_when_frame_interpolate_is_added() {
        let mut app = App::new();
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(start.0 + (end.0 - start.0) * t)
        }));
        app.finish();

        let entity = app.world_mut().spawn(FrameA(1.0)).id();
        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameA>>(entity)
                .is_none()
        );

        app.world_mut().entity_mut(entity).insert(FrameInterpolate);
        app.world_mut().run_schedule(FixedPostUpdate);

        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameA>>(entity)
                .is_some()
        );
    }

    /// Checks that adding a registered component later initializes its frame history.
    #[test]
    fn frame_update_inserts_history_when_component_is_added() {
        let mut app = App::new();
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(start.0 + (end.0 - start.0) * t)
        }));
        app.finish();

        let entity = app.world_mut().spawn(FrameInterpolate).id();
        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameA>>(entity)
                .is_none()
        );

        app.world_mut().entity_mut(entity).insert(FrameA(1.0));
        app.world_mut().run_schedule(FixedPostUpdate);

        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameA>>(entity)
                .is_some()
        );
    }

    /// Checks that spawning the marker and component together initializes frame history.
    #[test]
    fn frame_update_inserts_history_when_both_are_spawned() {
        let mut app = App::new();
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(start.0 + (end.0 - start.0) * t)
        }));
        app.finish();

        let entity = app.world_mut().spawn((FrameA(1.0), FrameInterpolate)).id();
        app.world_mut().run_schedule(FixedPostUpdate);

        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameA>>(entity)
                .is_some()
        );
    }

    /// Checks that a winning disabled rule blocks lower-priority frame-history creation.
    #[test]
    fn disabled_winner_does_not_insert_lower_rule_frame_history() {
        let mut app = App::new();
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(start.0 + (end.0 - start.0) * t)
        }));
        app.interpolate_with_priority_filtered::<FrameA, With<DisabledFrameHistory>>(
            100,
            InterpolationFns::disabled(),
        );
        app.finish();

        let entity = app
            .world_mut()
            .spawn((FrameA(1.0), DisabledFrameHistory, FrameInterpolate))
            .id();
        app.world_mut().run_schedule(FixedPostUpdate);

        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameA>>(entity)
                .is_none()
        );
    }

    /// Checks that every component owned by winning rules receives frame history.
    #[test]
    fn frame_update_initializes_all_histories_for_the_winning_rules() {
        let mut app = App::new();
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(start.0 + (end.0 - start.0) * t)
        }));
        app.interpolate_with::<FrameB>(InterpolationFns::no_history(|start, end, t| {
            FrameB(start.0 + (end.0 - start.0) * t)
        }));
        app.finish();

        let entity = app
            .world_mut()
            .spawn((FrameA(1.0), FrameB(2.0), FrameInterpolate))
            .id();
        app.world_mut().run_schedule(FixedPostUpdate);

        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameA>>(entity)
                .is_some()
        );
        assert!(
            app.world()
                .get::<FrameInterpolationHistory<FrameB>>(entity)
                .is_some()
        );
    }

    #[test]
    fn frame_bundle_interpolation_uses_tuple_rule() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(500));
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_bundle_with::<(FrameA, FrameB)>(InterpolationFns::no_history(bundle_lerp));
        app.finish();

        let entity = app
            .world_mut()
            .spawn((
                FrameInterpolate,
                FrameA(-1.0),
                FrameB(-1.0),
                FrameInterpolationHistory::<FrameA> {
                    previous_value: Some(FrameA(0.0)),
                    current_value: Some(FrameA(10.0)),
                },
                FrameInterpolationHistory::<FrameB> {
                    previous_value: Some(FrameB(0.0)),
                    current_value: Some(FrameB(20.0)),
                },
            ))
            .id();
        app.world_mut().clear_trackers();
        let first_changed = app
            .world()
            .entity(entity)
            .get_change_ticks::<FrameA>()
            .unwrap()
            .changed;
        let second_changed = app
            .world()
            .entity(entity)
            .get_change_ticks::<FrameB>()
            .unwrap()
            .changed;

        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(105.0)));
        assert_eq!(app.world().get::<FrameB>(entity), Some(&FrameB(210.0)));
        assert_ne!(
            app.world()
                .entity(entity)
                .get_change_ticks::<FrameA>()
                .unwrap()
                .changed,
            first_changed
        );
        assert_ne!(
            app.world()
                .entity(entity)
                .get_change_ticks::<FrameB>()
                .unwrap()
                .changed,
            second_changed
        );
    }

    /// Checks that a bundle can own histories while a component rule applies the live member.
    #[test]
    fn frame_bundle_maintains_history_while_component_rule_applies_to_live_member() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(500));
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(start.0 + (end.0 - start.0) * t)
        }));
        app.interpolate_bundle_with::<(FrameA, FrameB)>(InterpolationFns::no_history(bundle_lerp));
        app.finish();

        let entity = app
            .world_mut()
            .spawn((
                FrameInterpolate,
                FrameA(-1.0),
                FrameInterpolationHistory::<FrameA> {
                    previous_value: Some(FrameA(0.0)),
                    current_value: Some(FrameA(10.0)),
                },
                FrameInterpolationHistory::<FrameB> {
                    previous_value: Some(FrameB(0.0)),
                    current_value: Some(FrameB(20.0)),
                },
            ))
            .id();

        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(5.0)));
        assert!(!app.world().entity(entity).contains::<FrameB>());
    }

    #[test]
    fn frame_bundle_interpolation_receives_fixed_timestep() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(2)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(500));
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_bundle_with::<(FrameA, FrameB)>(InterpolationFns::no_history_with_context(
            bundle_context_lerp,
        ));
        app.finish();

        let entity = app
            .world_mut()
            .spawn((
                FrameInterpolate,
                FrameA(-1.0),
                FrameB(-1.0),
                FrameInterpolationHistory::<FrameA> {
                    previous_value: Some(FrameA(0.0)),
                    current_value: Some(FrameA(10.0)),
                },
                FrameInterpolationHistory::<FrameB> {
                    previous_value: Some(FrameB(0.0)),
                    current_value: Some(FrameB(20.0)),
                },
            ))
            .id();

        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(0.25)));
        assert_eq!(app.world().get::<FrameB>(entity), Some(&FrameB(2.0)));
    }

    #[test]
    fn frame_interpolation_reuses_no_history_interpolation_rule() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(250));
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(10.0 + start.0 + (end.0 - start.0) * t)
        }));
        app.finish();

        assert!(
            app.world()
                .components()
                .component_id::<ConfirmedHistory<FrameA>>()
                .is_some()
        );

        let entity = app
            .world_mut()
            .spawn((
                FrameInterpolate,
                FrameA(-1.0),
                FrameInterpolationHistory::<FrameA> {
                    previous_value: Some(FrameA(0.0)),
                    current_value: Some(FrameA(8.0)),
                },
            ))
            .id();
        app.world_mut().clear_trackers();
        let changed = app
            .world()
            .entity(entity)
            .get_change_ticks::<FrameA>()
            .unwrap()
            .changed;

        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(12.0)));
        assert_ne!(
            app.world()
                .entity(entity)
                .get_change_ticks::<FrameA>()
                .unwrap()
                .changed,
            changed
        );
    }

    #[test]
    fn frame_interpolation_reuses_history_only_interpolation_rule() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(250));
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin::default(),
            InterpolationMarkerPlugin,
        ));
        app.component::<FrameA>().replicate();
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(
            InterpolationFns::history_only()
                .interpolate(|start, end, t| FrameA(20.0 + start.0 + (end.0 - start.0) * t)),
        );
        app.finish();

        assert!(
            app.world()
                .components()
                .component_id::<ConfirmedHistory<FrameA>>()
                .is_some()
        );

        let entity = app
            .world_mut()
            .spawn((
                FrameInterpolate,
                FrameA(-1.0),
                FrameInterpolationHistory::<FrameA> {
                    previous_value: Some(FrameA(4.0)),
                    current_value: Some(FrameA(12.0)),
                },
            ))
            .id();

        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(26.0)));
    }

    #[test]
    fn frame_interpolation_uses_filtered_rule_for_frame_interpolate_marker() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(250));
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|start, end, t| {
            FrameA(10.0 + start.0 + (end.0 - start.0) * t)
        }));
        app.interpolate_with_priority_filtered::<FrameA, With<FrameInterpolate>>(
            100,
            InterpolationFns::no_history(|start, end, t| {
                FrameA(20.0 + start.0 + (end.0 - start.0) * t)
            }),
        );
        app.finish();

        let entity = app
            .world_mut()
            .spawn((
                FrameInterpolate,
                FrameA(-1.0),
                FrameInterpolationHistory::<FrameA> {
                    previous_value: Some(FrameA(4.0)),
                    current_value: Some(FrameA(12.0)),
                },
            ))
            .id();

        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(26.0)));
    }

    #[test]
    fn frame_interpolation_supports_sparse_set_live_component() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_millis(500));
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<SparseFrame>(InterpolationFns::no_history(|start, end, t| {
            SparseFrame(start.0 + (end.0 - start.0) * t)
        }));
        app.finish();

        let entity = app
            .world_mut()
            .spawn((FrameInterpolate, SparseFrame(1.0)))
            .id();

        app.world_mut().run_schedule(FixedPostUpdate);
        assert_eq!(
            app.world()
                .get::<FrameInterpolationHistory<SparseFrame>>(entity)
                .and_then(|history| history.current_value.as_ref()),
            Some(&SparseFrame(1.0))
        );

        app.world_mut().entity_mut(entity).insert(SparseFrame(3.0));
        app.world_mut().run_schedule(FixedPostUpdate);
        let history = app
            .world()
            .get::<FrameInterpolationHistory<SparseFrame>>(entity)
            .unwrap();
        assert_eq!(history.previous_value, Some(SparseFrame(1.0)));
        assert_eq!(history.current_value, Some(SparseFrame(3.0)));
        app.world_mut().clear_trackers();
        let changed = app
            .world()
            .entity(entity)
            .get_change_ticks::<SparseFrame>()
            .unwrap()
            .changed;

        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            app.world().get::<SparseFrame>(entity),
            Some(&SparseFrame(2.0))
        );
        assert_ne!(
            app.world()
                .entity(entity)
                .get_change_ticks::<SparseFrame>()
                .unwrap()
                .changed,
            changed
        );

        app.world_mut().run_schedule(RunFixedMainLoop);
        assert_eq!(
            app.world().get::<SparseFrame>(entity),
            Some(&SparseFrame(3.0))
        );
    }

    #[test]
    fn frame_interpolation_updates_change_detection_for_table_and_sparse_set_components() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs(1)));
        app.add_plugins(FrameInterpolationPlugin);
        app.interpolate_with::<FrameA>(InterpolationFns::no_history(|_, end, _| end));
        app.interpolate_with::<SparseFrame>(InterpolationFns::no_history(|_, end, _| end));
        app.init_resource::<ObservedFrameInterpolationChanges>();
        app.add_systems(
            PostUpdate,
            observe_frame_interpolation_changes.after(FrameInterpolationSystems::Interpolate),
        );
        app.finish();

        app.world_mut().spawn((
            FrameInterpolate,
            FrameA(0.0),
            SparseFrame(0.0),
            FrameInterpolationHistory::<FrameA> {
                previous_value: Some(FrameA(0.0)),
                current_value: Some(FrameA(1.0)),
            },
            FrameInterpolationHistory::<SparseFrame> {
                previous_value: Some(SparseFrame(0.0)),
                current_value: Some(SparseFrame(1.0)),
            },
        ));

        // Warm up the observer so the next run cannot observe the components' spawn ticks.
        app.world_mut().run_schedule(PostUpdate);
        *app.world_mut()
            .resource_mut::<ObservedFrameInterpolationChanges>() = Default::default();

        app.world_mut().run_schedule(PostUpdate);

        let observed = app.world().resource::<ObservedFrameInterpolationChanges>();
        assert!(observed.table);
        assert!(observed.sparse_set);
    }
}
