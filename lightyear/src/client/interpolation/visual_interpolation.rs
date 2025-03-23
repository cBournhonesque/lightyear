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
//! To enable VisualInterpolation:
//! - you will have to register an interpolation function for the component in the protocol
//! - VisualInterpolation is not enabled by default, you have to add the plugin manually
//! ```rust,no_run,ignore
//! # use crate::tests::protocol::*;
//! use lightyear::prelude::client::VisualInterpolationPlugin;
//! let mut app = bevy::app::App::new();
//! app.add_plugins(VisualInterpolationPlugin::<Component1>::default());
//! ```
//! - To enable VisualInterpolation on a given entity, you need to add the `VisualInterpolateStatus` component to it manually
//! ```rust,no_run,ignore
//! fn spawn_entity(mut commands: Commands) {
//!     commands.spawn().insert(VisualInterpolateStatus::<Component1>::default());
//! }
//! ```

// TODO: in post-update, interpolate the visual state of the game between with 1 tick of delay.
// - we need to store the component values of the previous tick
// - then in PostUpdate (visual interpolation) we interpolate between the previous tick and the current tick using the overstep
// - in PreUpdate, we restore the component value to the previous tick values

use bevy::prelude::*;
use bevy::transform::TransformSystem::TransformPropagate;
use tracing::trace;

use crate::client::components::SyncComponent;
use crate::prelude::client::{is_in_rollback, Correction, InterpolationSet, PredictionSet};
use crate::prelude::{ComponentRegistry, MainSet, TickManager, TimeManager};

pub struct VisualInterpolationPlugin<C: SyncComponent> {
    _marker: core::marker::PhantomData<C>,
}

impl<C: SyncComponent> Default for VisualInterpolationPlugin<C> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C: SyncComponent> Plugin for VisualInterpolationPlugin<C> {
    fn build(&self, app: &mut App) {
        // SETS
        app.configure_sets(
            PreUpdate,
            // make sure that we restore the actual component value before we perform a rollback check
            (
                InterpolationSet::RestoreVisualInterpolation,
                // the correct value to avoid rollbacks is the corrected value
                PredictionSet::RestoreVisualCorrection,
                PredictionSet::CheckRollback,
            )
                .chain(),
        );
        // We don't run UpdateVisualInterpolationState in rollback because:
        // - in case of rollback, that would mean we repeatedly interpolate the component for no reason
        // - in case of correction, we would be interpolating between CorrectedValue (last value during rollback) and CorrectInterpolatedValue (first value
        //   after Correction)
        app.configure_sets(
            FixedLast,
            InterpolationSet::UpdateVisualInterpolationState.run_if(not(is_in_rollback)),
        );
        app.configure_sets(
            PostUpdate,
            InterpolationSet::VisualInterpolation
                .before(TransformPropagate)
                // we don't want the visual interpolation value to be the one replicated!
                .after(MainSet::Send),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            restore_from_visual_interpolation::<C>
                .in_set(InterpolationSet::RestoreVisualInterpolation),
        );
        app.add_systems(
            FixedLast,
            update_visual_interpolation_status::<C>
                .in_set(InterpolationSet::UpdateVisualInterpolationState),
        );
        app.add_systems(
            PostUpdate,
            visual_interpolation::<C>.in_set(InterpolationSet::VisualInterpolation),
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
pub struct VisualInterpolateStatus<C: Component> {
    /// If true, every change of the component due to visual interpolation will trigger change detection
    /// (this can be useful for `Transform` to trigger a `TransformPropagate` system)
    pub trigger_change_detection: bool,
    /// Value of the component at the previous tick
    pub previous_value: Option<C>,
    /// Value of the component at the current tick
    pub current_value: Option<C>,
}

// Manual implementation because we don't want to force `Component` to have a `Default` bound
impl<C: Component> Default for VisualInterpolateStatus<C> {
    fn default() -> Self {
        Self {
            trigger_change_detection: false,
            previous_value: None,
            current_value: None,
        }
    }
}

// TODO: explore how we could allow this for non-marker components, user would need to specify the interpolation function?
//  (to avoid orphan rule)
/// Currently we will only support components that are present in the protocol and have a SyncMetadata implementation
pub(crate) fn visual_interpolation<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    tick_manager: Res<TickManager>,
    time_manager: Res<TimeManager>,
    mut query: Query<(&mut C, &VisualInterpolateStatus<C>)>,
) {
    let kind = core::any::type_name::<C>();
    let tick = tick_manager.tick();
    let overstep = time_manager.overstep();
    for (mut component, interpolate_status) in query.iter_mut() {
        let Some(previous_value) = &interpolate_status.previous_value else {
            trace!(?kind, "No previous value, skipping visual interpolation");
            continue;
        };
        let Some(current_value) = &interpolate_status.current_value else {
            trace!(?kind, "No current value, skipping visual interpolation");
            continue;
        };
        trace!(
            ?kind,
            ?tick,
            ?overstep,
            "Visual interpolation of fixed-update component!"
        );
        if !interpolate_status.trigger_change_detection {
            *component.bypass_change_detection() =
                component_registry.interpolate(previous_value, current_value, overstep);
        } else {
            *component = component_registry.interpolate(previous_value, current_value, overstep);
        }
    }
}

/// Update the previous and current tick values.
/// Runs in FixedUpdate after FixedUpdate::Main (where the component values are updated)
pub(crate) fn update_visual_interpolation_status<C: SyncComponent>(
    mut query: Query<(Ref<C>, &mut VisualInterpolateStatus<C>)>,
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
        trace!("updating interpolate status current_value");
        interpolate_status.current_value = Some(component.clone());
    }
}

/// Restore the component value to the non-interpolated value
pub(crate) fn restore_from_visual_interpolation<C: SyncComponent>(
    // if correction is enabled, we will restore the value from the Correction component
    mut query: Query<(&mut C, &mut VisualInterpolateStatus<C>), Without<Correction<C>>>,
) {
    let kind = core::any::type_name::<C>();
    for (mut component, interpolate_status) in query.iter_mut() {
        if let Some(current_value) = &interpolate_status.current_value {
            trace!(?kind, "Restoring visual interpolation");
            *component.bypass_change_detection() = current_value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::components::Confirmed;
    use crate::client::config::ClientConfig;
    use crate::client::easings::ease_out_quad;
    use crate::client::prediction::rollback::test_utils::received_confirmed_update;
    use crate::client::prediction::Predicted;
    use crate::prelude::client::PredictionConfig;
    use crate::prelude::{SharedConfig, TickConfig};
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use approx::assert_relative_eq;

    use core::time::Duration;

    #[derive(Resource, Debug)]
    pub struct Toggle(bool);

    fn setup(tick_duration: Duration, frame_duration: Duration) -> (BevyStepper, Entity) {
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        // we create the stepper manually to not run init()
        let mut stepper = BevyStepper::new(shared_config, ClientConfig::default(), frame_duration);
        stepper
            .client_app
            .add_systems(FixedUpdate, fixed_update_increment);
        stepper.client_app.world_mut().insert_resource(Toggle(true));
        stepper
            .client_app
            .add_plugins(VisualInterpolationPlugin::<ComponentSyncModeFull>::default());
        let entity = stepper
            .client_app
            .world_mut()
            .spawn((
                ComponentSyncModeFull(0.0),
                VisualInterpolateStatus::<ComponentSyncModeFull>::default(),
            ))
            .id();
        stepper.build();
        (stepper, entity)
    }

    fn fixed_update_increment(
        mut query: Query<&mut ComponentSyncModeFull>,
        mut query_correction: Query<&mut ComponentCorrection>,
        enabled: Res<Toggle>,
    ) {
        if enabled.0 {
            for mut component in query.iter_mut() {
                component.0 += 1.0;
            }
            for mut component in query_correction.iter_mut() {
                component.0 += 1.0;
            }
        }
    }

    #[test]
    fn test_shorter_tick_normal() {
        let (mut stepper, entity) = setup(Duration::from_millis(9), Duration::from_millis(12));

        stepper.frame_step();
        // TODO: should we not show the component at all until we have enough to interpolate?
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.0
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: None,
                current_value: Some(ComponentSyncModeFull(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.33,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.66,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(1.0)),
                current_value: Some(ComponentSyncModeFull(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.66,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            3.00,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(3.0)),
                current_value: Some(ComponentSyncModeFull(4.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.00,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            4.33,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(4.0)),
                current_value: Some(ComponentSyncModeFull(5.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.33,
            max_relative = 0.1
        );
    }

    #[test]
    fn test_shorter_tick_unchanged() {
        let (mut stepper, entity) = setup(Duration::from_millis(9), Duration::from_millis(12));

        stepper.frame_step();
        // TODO: should we not show the component at all until we have enough to interpolate?
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.0
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: None,
                current_value: Some(ComponentSyncModeFull(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.33,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.66,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(1.0)),
                current_value: Some(ComponentSyncModeFull(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.66,
            max_relative = 0.1
        );

        stepper.client_app.world_mut().resource_mut::<Toggle>().0 = false;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.00,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.00,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.33,
            max_relative = 0.1
        );
        stepper.client_app.world_mut().resource_mut::<Toggle>().0 = true;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.66,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: Some(ComponentSyncModeFull(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.66,
            max_relative = 0.1
        );
    }

    #[test]
    fn test_shorter_frame_normal() {
        let (mut stepper, entity) = setup(Duration::from_millis(12), Duration::from_millis(9));

        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            0.0
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: None,
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.75,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: None,
                current_value: Some(ComponentSyncModeFull(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.5,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.25,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(1.0)),
                current_value: Some(ComponentSyncModeFull(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.25,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: Some(ComponentSyncModeFull(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.0,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.75,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: Some(ComponentSyncModeFull(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.75,
            max_relative = 0.1
        );
    }

    #[test]
    fn test_shorter_frame_unchanged() {
        let (mut stepper, entity) = setup(Duration::from_millis(12), Duration::from_millis(9));

        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            0.0
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: None,
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.75,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: None,
                current_value: Some(ComponentSyncModeFull(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.5,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            1.25,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(1.0)),
                current_value: Some(ComponentSyncModeFull(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.25,
            max_relative = 0.1
        );

        stepper.client_app.world_mut().resource_mut::<Toggle>().0 = false;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.0,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.75,
            max_relative = 0.1
        );

        stepper.client_app.world_mut().resource_mut::<Toggle>().0 = true;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<ComponentSyncModeFull>()
                .unwrap()
                .0,
            2.5,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity)
                .get::<VisualInterpolateStatus<ComponentSyncModeFull>>()
                .unwrap(),
            &VisualInterpolateStatus {
                trigger_change_detection: false,
                previous_value: Some(ComponentSyncModeFull(2.0)),
                current_value: Some(ComponentSyncModeFull(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world()
                .resource::<TimeManager>()
                .overstep(),
            0.5,
            max_relative = 0.1
        );
    }

    fn setup_predicted(
        tick_duration: Duration,
        frame_duration: Duration,
    ) -> (BevyStepper, Entity, Entity) {
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let client_config = ClientConfig {
            prediction: PredictionConfig {
                correction_ticks_factor: 1.0,
                ..default()
            },
            ..default()
        };
        // we create the stepper manually to not run init()
        let mut stepper = BevyStepper::new(shared_config, client_config, frame_duration);
        stepper.client_app.world_mut().insert_resource(Toggle(true));
        stepper
            .client_app
            .add_systems(FixedUpdate, fixed_update_increment);
        stepper
            .client_app
            .add_plugins(VisualInterpolationPlugin::<ComponentCorrection>::default());
        stepper.build();
        stepper.init();
        let tick = stepper.client_tick();

        let confirmed = stepper
            .client_app
            .world_mut()
            .spawn(Confirmed {
                tick,
                ..Default::default()
            })
            .id();
        let predicted = stepper
            .client_app
            .world_mut()
            .spawn((
                Predicted {
                    confirmed_entity: Some(confirmed),
                },
                VisualInterpolateStatus::<ComponentCorrection>::default(),
            ))
            .id();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .get_mut::<Confirmed>()
            .unwrap()
            .predicted = Some(predicted);
        stepper.frame_step();
        (stepper, confirmed, predicted)
    }

    /// Test that visual interpolation works with predicted entities
    /// that get corrected
    #[test]
    fn test_visual_interpolation_and_correction() {
        let (mut stepper, confirmed, predicted) =
            setup_predicted(Duration::from_millis(12), Duration::from_millis(9));

        // create a rollback situation (component absent from predicted history)
        let original_tick = stepper.client_tick();
        let rollback_tick = original_tick - 5;
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentCorrection(1.0));
        let tick = stepper.client_tick();
        received_confirmed_update(&mut stepper, confirmed, rollback_tick);

        stepper.frame_step();

        // 1. component gets synced from confirmed to predicted
        // 2. check rollback is triggered because Confirmed changed
        // 3. on prepare_rollback, we insert the component with Correction
        // 4. we do a rollback to update the component to the correct value
        //    - the predicted value is 1.0
        //    - the corrected value is 7.0
        //    - the correct_interpolation value is 20% of the way, so we should see 1.0 + 0.2 * (7.0 - 1.0) = 2.2
        // 5. visual interpolation should record the 2 values, so 1.0 and 2.2, and visually interpolate between them
        //    Rollback saves the overstep from before the rollback, so the overstep should still be 0.75
        //    NOTE: actually the overstep might not be 0.75 because the SyncPlugin modifies the virtual time!!!

        // interpolate 20% of the way
        let current_visual = Some(ComponentCorrection(1.0 + ease_out_quad(0.2) * (7.0 - 1.0)));
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Correction<ComponentCorrection>>(predicted)
                .unwrap(),
            &Correction::<ComponentCorrection> {
                original_prediction: ComponentCorrection(1.0),
                original_tick,
                final_correction_tick: original_tick + (original_tick - rollback_tick),
                current_visual: current_visual.clone(),
                current_correction: Some(ComponentCorrection(7.0)),
            }
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<VisualInterpolateStatus<ComponentCorrection>>()
                .unwrap(),
            &VisualInterpolateStatus::<ComponentCorrection> {
                trigger_change_detection: false,
                // TODO: maybe we'd like to interpolate from 1.0 here? we could have custom logic where
                //  post-rollback if previous_value is None and Correction is enabled, we set previous_value to original_prediction?
                previous_value: None,
                current_value: current_visual,
            }
        );
    }
}
