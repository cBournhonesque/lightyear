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
}

pub(crate) fn restore_frame_interpolation(
    mut frame_world: FrameInterpolationWorld,
    interpolation_registry: Res<InterpolationRegistry>,
) {
    frame_world.update_archetypes(&interpolation_registry);
    let world = frame_world.world;
    for (archetype, cached_archetype) in frame_world.iter_archetypes() {
        for component in cached_archetype.history_components() {
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
        for component in cached_archetype.history_components() {
            (component.update_frame_history())(world, archetype, component, &mut deferred_apply);
        }
    }

    deferred_apply.apply(&mut commands);
}

pub(crate) fn apply_frame_interpolation(
    time: Res<Time<Fixed>>,
    mut frame_world: FrameInterpolationWorld,
    interpolation_registry: Res<InterpolationRegistry>,
    mut commands: Commands,
) {
    let mut deferred_apply = DeferredEntityCommands::default();
    let ctx = FrameInterpolationContext {
        overstep: time.overstep_fraction().clamp(0.0, 1.0),
    };

    frame_world.update_archetypes(&interpolation_registry);
    let world = frame_world.world;
    for (archetype, cached_archetype) in frame_world.iter_archetypes() {
        for component in cached_archetype.apply_rules() {
            (component.apply_frame_interpolation())(
                world,
                archetype,
                &interpolation_registry,
                component.rule_id(),
                ctx,
                cached_archetype.skip_interpolation(),
                &mut deferred_apply,
            );
        }
    }

    deferred_apply.apply(&mut commands);
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
mod tests {
    use super::*;
    use bevy_app::{App, FixedPostUpdate, PostUpdate, RunFixedMainLoop};
    use core::time::Duration;
    use lightyear_core::prelude::ConfirmedHistory;
    use lightyear_interpolation::registry::AppInterpolationExt;
    use lightyear_interpolation::rules::InterpolationFns;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct FrameA(f32);

    #[derive(Component, Clone, Debug, PartialEq)]
    struct FrameB(f32);

    #[derive(Component, Clone, Debug, PartialEq)]
    #[component(storage = "SparseSet")]
    struct SparseFrame(f32);

    fn bundle_lerp(start: (FrameA, FrameB), end: (FrameA, FrameB), t: f32) -> (FrameA, FrameB) {
        (
            FrameA(100.0 + start.0.0 + (end.0.0 - start.0.0) * t),
            FrameB(200.0 + start.1.0 + (end.1.0 - start.1.0) * t),
        )
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

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(105.0)));
        assert_eq!(app.world().get::<FrameB>(entity), Some(&FrameB(210.0)));
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

        assert!(
            app.world()
                .components()
                .component_id::<ConfirmedHistory<FrameA>>()
                .is_none()
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

        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().get::<FrameA>(entity), Some(&FrameA(12.0)));
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

        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            app.world().get::<SparseFrame>(entity),
            Some(&SparseFrame(2.0))
        );

        app.world_mut().run_schedule(RunFixedMainLoop);
        assert_eq!(
            app.world().get::<SparseFrame>(entity),
            Some(&SparseFrame(3.0))
        );
    }
}
