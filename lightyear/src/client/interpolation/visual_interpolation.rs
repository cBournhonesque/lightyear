//! This module is not related to the interpolating between server updates.
//! Instead it is responsible for interpolating between FixedUpdate ticks during the Update state.
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

// TODO: in post-update, interpolate the visual state of the game between with 1 tick of delay.
// - we need to store the component values of the previous tick
// - then in PostUpdate (visual interpolation) we interpolate between the previous tick and the current tick using the overstep
// - in PreUpdate, we restore the component value to the previous tick values

use crate::_reexport::ComponentProtocol;
use crate::client::components::{SyncComponent, SyncMetadata};
use crate::prelude::{Protocol, Tick, TickManager, TimeManager};
use bevy::prelude::{Component, DetectChanges, DetectChangesMut, Query, Ref, Res};
use tracing::info;

// TODO: we might want to add this automatically to some entities that are predicted?
/// Component that stores the previous value of a component for visual interpolation
/// For now we will only use this to interpolate components that are updated during the FixedUpdate schedule.
/// Hence, some values are not included in the struct:
/// - start_tick = current_tick - 1
/// - start_value = previous_value
/// - end_tick = current_tick
/// - end_value = current_value
/// - overstep = Time<Fixed>.overstep_percentage() = TimeManager.overstep()
#[derive(Component, PartialEq, Debug)]
pub struct VisualInterpolateStatus<C: Component> {
    /// Value of the component at the previous tick
    pub previous_value: Option<C>,
    /// Value of the component at the current tick
    pub current_value: Option<C>,
}

// Manual implementation because we don't want to force `Component` to have a `Default` bound
impl<C: Component> Default for VisualInterpolateStatus<C> {
    fn default() -> Self {
        Self {
            previous_value: None,
            current_value: None,
        }
    }
}

// TODO: test the visual interpolation in 2 settings
//  - multiple ticks in one frame
//  - no ticks in one frame
// T1 T2 F  F T3 T4 F

// F T1 T2 F T3 F T4 T5 F

/// Marker component to indicate that this entity will be visually interpolated
#[derive(Component, Debug)]
pub struct VisualInterpolateMarker;

// TODO: explore how we could allow this for non-marker components, user would need to specify the interpolation function?
//  (to avoid orphan rule)
/// Currently we will only support components that are present in the protocol and have a SyncMetadata implementation
pub(crate) fn visual_interpolation<C: SyncComponent, P: Protocol>(
    tick_manager: Res<TickManager>,
    time_manager: Res<TimeManager>,
    mut query: Query<(&mut C, &VisualInterpolateStatus<C>)>,
) where
    P::Components: SyncMetadata<C>,
{
    let kind = C::type_name();
    let tick = tick_manager.tick();
    let overstep = time_manager.overstep();
    for (mut component, mut interpolate_status) in query.iter_mut() {
        // TODO: think about how we can avoid running this all the time if the component is not changed
        // if !component.is_changed() {
        //     info!(
        //         ?kind,
        //         "Component is not changed, skipping visual interpolation"
        //     );
        //     continue;
        // }
        let Some(previous_value) = &interpolate_status.previous_value else {
            info!(?kind, "No previous value, skipping visual interpolation");
            continue;
        };
        let Some(current_value) = &interpolate_status.current_value else {
            info!(?kind, "No current value, skipping visual interpolation");
            continue;
        };
        info!(
            ?kind,
            ?tick,
            ?overstep,
            // ?previous_value,
            // current_value = ?component.as_ref(),
            "Visual interpolation of fixed-update component!"
        );
        *component.bypass_change_detection() =
            P::Components::lerp(previous_value, current_value, overstep);
        // interpolate_status.previous_value = Some(current_value);
    }
}

// TODO: handle edge states
/// Update the previous and current tick values.
/// Runs in FixedUpdate after FixedUpdate::Main (where the component values are updated)
pub(crate) fn update_visual_interpolation_status<C: SyncComponent>(
    mut query: Query<(Ref<C>, &mut VisualInterpolateStatus<C>)>,
) {
    for (component, mut interpolate_status) in query.iter_mut() {
        if !component.is_changed() {
            info!("not updating interpolate status because component did not change");
            continue;
        }
        info!("updating interpolate status");
        if let Some(current_value) = interpolate_status.current_value.take() {
            interpolate_status.previous_value = Some(current_value);
        }
        interpolate_status.current_value = Some(component.clone());
    }
}

/// Restore the component value to the non-interpolated value
pub(crate) fn restore_from_visual_interpolation<C: SyncComponent>(
    mut query: Query<(&mut C, &mut VisualInterpolateStatus<C>)>,
) {
    let kind = C::type_name();
    for (mut component, mut interpolate_status) in query.iter_mut() {
        // TODO: do this only if we actually did visual interpolation
        // if interpolate_status.is_changed() {
        if let Some(current_value) = &interpolate_status.current_value {
            info!(?kind, "Restoring visual interpolation");
            *component.bypass_change_detection() = current_value.clone();
        }
        // }
    }
}
