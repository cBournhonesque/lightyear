//! This module is not related to the interpolating between server updates.
//! Instead, it is responsible for interpolating between FixedUpdate ticks during the Update state.
//!
//! Usually, the simulation is run during the FixedUpdate schedule so that it doesn't depend on frame rate.
//! This can cause some visual inconsistencies (jitter) because the frames (Update schedule) don't correspond exactly to
//! the FixedUpdate schedule: there can be frames with several fixed-update ticks, and some frames with no fixed-update ticks.
//!
//! To solve this, we will visually display the state of the game with 1 tick of delay
//! For example if on the Update state we have an overstep of 0.7 and the current tick is 10,
//! we will display the state of the game interpolated at 0.7 between tick 9 and tick 10.
//!
//! Another way to solve this would to run an extra 'partial' simulation step with 0.7 dt and use this for the visual state.
//!
//! To enable FrameInterpolation:
//! - you will have to register an interpolation function for the component in the protocol
//! - FrameInterpolation is not enabled by default, you have to add the plugin manually
//! - To enable VisualInterpolation on a given entity, you need to add the `FrameInterpolate` component to it manually
//! ```rust
//! # use lightyear_frame_interpolation::prelude::*;
//! # use bevy_app::App;
//! # use bevy_ecs::prelude::*;
//!
//! # #[derive(Component, PartialEq, Clone, Debug)]
//! # struct Component1;
//!
//! let mut app = App::new();
//! app.add_plugins(FrameInterpolationPlugin::<Component1>::default());
//!
//! fn spawn_entity(mut commands: Commands) {
//!     commands.spawn(FrameInterpolate::<Component1>::default());
//! }
//! ```

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod prelude {
    pub use crate::{FrameInterpolate, FrameInterpolationPlugin, FrameInterpolationSet};
}

// #[cfg(test)]
// mod tests;

// TODO: in post-update, interpolate the visual state of the game between with 1 tick of delay.
// - we need to store the component values of the previous tick
// - then in PostUpdate (visual interpolation) we interpolate between the previous tick and the current tick using the overstep
// - in PreUpdate, we restore the component value to the previous tick values
use bevy_app::{App, FixedLast, Plugin, PostUpdate, RunFixedMainLoop, RunFixedMainLoopSystem};
use bevy_ecs::{
    change_detection::{DetectChanges, DetectChangesMut},
    component::{Component, Mutable},
    query::With,
    schedule::{IntoScheduleConfigs, SystemSet, common_conditions::not},
    system::{Query, Res, Single},
    world::Ref,
};
use bevy_time::{Fixed, Time};
use bevy_transform::TransformSystem;
use core::fmt::Debug;
use lightyear_connection::client::Client;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::timeline::is_in_rollback;
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_replication::prelude::ReplicationSet;
use tracing::trace;

/// System sets used by the `FrameInterpolationPlugin`.
///
/// These sets help order the systems responsible for:
/// - Restoring the actual component values before other game logic.
/// - Updating the history of component values used for interpolation.
/// - Performing the visual interpolation itself.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum FrameInterpolationSet {
    /// Restore the correct component values when we start the FixedMainLoop
    Restore,
    /// Update the previous/current component values used for visual interpolation
    Update,
    /// Interpolate the visual state of the game with 1 tick of delay
    Interpolate,
}

/// Bevy plugin to enable visual interpolation for a specific component `C`.
///
/// This plugin adds systems to store the state of component `C` at each `FixedUpdate` tick
/// and then interpolate between the previous and current tick's state during the `PostUpdate`
/// schedule, using the `Time<Fixed>::overstep_percentage`. This helps smooth
/// visuals when the rendering framerate doesn't align perfectly with the fixed simulation rate.
///
/// To use this, the component `C` must implement `Component<Mutability=Mutable> + Clone` and have an
/// interpolation function registered in the `InterpolationRegistry`.
/// You also need to add the `FrameInterpolate<C>` component to entities
/// for which you want to enable this visual interpolation.
pub struct FrameInterpolationPlugin<C> {
    _marker: core::marker::PhantomData<C>,
}

impl<C> Default for FrameInterpolationPlugin<C> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C: Component<Mutability = Mutable> + Clone + Debug> Plugin for FrameInterpolationPlugin<C> {
    fn build(&self, app: &mut App) {
        // SETS
        app.configure_sets(
            RunFixedMainLoop,
            FrameInterpolationSet::Restore.in_set(RunFixedMainLoopSystem::BeforeFixedMainLoop),
        );
        // We don't run UpdateVisualInterpolationState in rollback because we that would be a waste.
        // At the end of rollback, we have a system in lightyear_prediction that manually sets the FrameInterpolate component.
        app.configure_sets(
            FixedLast,
            FrameInterpolationSet::Update.run_if(not(is_in_rollback)),
        );
        app.configure_sets(
            PostUpdate,
            FrameInterpolationSet::Interpolate
                .before(TransformSystem::TransformPropagate)
                // we don't want the visual interpolation value to be the one replicated!
                .after(ReplicationSet::Send),
        );

        // SYSTEMS
        app.add_systems(
            RunFixedMainLoop,
            restore_from_visual_interpolation::<C>.in_set(FrameInterpolationSet::Restore),
        );
        app.add_systems(
            FixedLast,
            update_visual_interpolation_status::<C>.in_set(FrameInterpolationSet::Update),
        );
        app.add_systems(
            PostUpdate,
            visual_interpolation::<C>.in_set(FrameInterpolationSet::Interpolate),
        );
    }
}

// TODO: we might want to add this automatically to some entities that are predicted?
/// Component that stores the previous value of a component for visual interpolation
/// For now we will only use this to interpolate components that are updated during the FixedUpdate schedule.
/// Hence, some values are not included in the struct:
/// - start_tick = current_tick - 1
/// - start_value = previous_value
/// - end_tick = current_tick
/// - end_value = current_value
/// - overstep = `Time<Fixed>`.overstep_percentage() = TimeManager.overstep()
#[derive(Component, PartialEq, Debug)]
pub struct FrameInterpolate<C: Component> {
    /// If true, every change of the component due to visual interpolation will trigger change detection
    /// (this can be useful for `Transform` to trigger a `TransformPropagate` system)
    pub trigger_change_detection: bool,
    /// Value of the component at the previous tick
    pub previous_value: Option<C>,
    /// Value of the component at the current tick
    pub current_value: Option<C>,
}

// Manual implementation because we don't want to force `Component` to have a `Default` bound
impl<C: Component> Default for FrameInterpolate<C> {
    fn default() -> Self {
        Self {
            trigger_change_detection: false,
            previous_value: None,
            current_value: None,
        }
    }
}

/// Currently we will only support components that are present in the protocol and have a SyncMetadata implementation
pub(crate) fn visual_interpolation<C: Component<Mutability = Mutable> + Clone + Debug>(
    time: Res<Time<Fixed>>,
    registry: Res<InterpolationRegistry>,
    timeline: Single<&LocalTimeline, With<Client>>,
    mut query: Query<(&mut C, &FrameInterpolate<C>)>,
) {
    let kind = core::any::type_name::<C>();
    let tick = timeline.now.tick;
    // TODO: how should we get the overstep? the LocalTimeline is only incremented during FixedUpdate so has an overstep of 0.0
    //  the InputTimeline seems to have an overstep, but it doesn't match the Time<Fixed> overstep
    let overstep = time.overstep_fraction();
    for (mut component, interpolate_status) in query.iter_mut() {
        let Some(previous_value) = &interpolate_status.previous_value else {
            trace!(?kind, "No previous value, skipping visual interpolation");
            continue;
        };
        let Some(current_value) = &interpolate_status.current_value else {
            trace!(?kind, "No current value, skipping visual interpolation");
            continue;
        };
        let interpolated =
            registry.interpolate(previous_value.clone(), current_value.clone(), overstep);
        trace!(
            ?kind,
            ?tick,
            ?previous_value,
            ?current_value,
            ?overstep,
            ?interpolated,
            "Visual interpolation applied"
        );
        if !interpolate_status.trigger_change_detection {
            *component.bypass_change_detection() = interpolated;
        } else {
            *component = interpolated;
        }
    }
}

/// Update the previous and current tick values.
/// Runs in FixedUpdate after FixedUpdate::Main (where the component values are updated)
pub(crate) fn update_visual_interpolation_status<
    C: Component<Mutability = Mutable> + Clone + Debug,
>(
    mut query: Query<(Ref<C>, &mut FrameInterpolate<C>)>,
) {
    for (component, mut interpolate_status) in query.iter_mut() {
        if let Some(current_value) = interpolate_status.current_value.take() {
            interpolate_status.previous_value = Some(current_value);
        }
        if !component.is_changed() {
            trace!(
                "not updating interpolate status current value because component did not change"
            );
            continue;
        }
        interpolate_status.current_value = Some(component.clone());
        trace!(
            ?interpolate_status,
            "updating interpolate status current_value"
        );
    }
}

/// Restore the component value to the non-interpolated value
pub(crate) fn restore_from_visual_interpolation<
    C: Component<Mutability = Mutable> + Clone + Debug,
>(
    mut query: Query<(&mut C, &mut FrameInterpolate<C>)>,
) {
    let kind = core::any::type_name::<C>();
    for (mut component, interpolate_status) in query.iter_mut() {
        if let Some(current_value) = &interpolate_status.current_value {
            trace!(
                ?kind,
                ?component,
                ?current_value,
                "Restoring visual interpolation"
            );
            *component.bypass_change_detection() = current_value.clone();
        }
    }
}
