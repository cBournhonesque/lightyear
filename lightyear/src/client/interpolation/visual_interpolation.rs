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
//!
//! This module currently has some caveats:
//! - the systems are only compatible with components that are present in the protocol and have a SyncMetadata implementation
//!   (because the InterpolatorFn is associated with the protocol, not the component itself, to circumvent the orphan rule)
//! - VisualInterpolation is not enabled by default, you have to add the plugin manually
//! ```rust,no_run,ignore
//! # use crate::tests::protocol::*;
//! use lightyear::prelude::client::VisualInterpolationPlugin;
//! let mut app = bevy::app::App::new();
//! app.add_plugins(VisualInterpolationPlugin::<Component1, MyProtocol>::default());
//! ```
//! - To enable VisualInterpolation on a given entity, you need to add the `VisualInterpolateStatus` component to it manually
//! ```rust,no_run,ignore
//! fn spawn_entity(mut commands: Commands) {
//!     commands.spawn().insert(VisualInterpolateState::<Component1>::default());
//! }
//! ```

// TODO: in post-update, interpolate the visual state of the game between with 1 tick of delay.
// - we need to store the component values of the previous tick
// - then in PostUpdate (visual interpolation) we interpolate between the previous tick and the current tick using the overstep
// - in PreUpdate, we restore the component value to the previous tick values

use bevy::prelude::*;

use crate::_reexport::ComponentProtocol;
use crate::client::components::{ComponentSyncMode, SyncComponent, SyncMetadata};
use crate::prelude::client::InterpolationSet;
use crate::prelude::{Protocol, TickManager, TimeManager};

pub struct VisualInterpolationPlugin<C: SyncComponent, P: Protocol>
where
    P::Components: SyncMetadata<C>,
{
    _marker: std::marker::PhantomData<(C, P)>,
}

impl<C: SyncComponent, P: Protocol> Default for VisualInterpolationPlugin<C, P>
where
    P::Components: SyncMetadata<C>,
{
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<C: SyncComponent, P: Protocol> Plugin for VisualInterpolationPlugin<C, P>
where
    P::Components: SyncMetadata<C>,
{
    fn build(&self, app: &mut App) {
        // SETS
        app.configure_sets(PreUpdate, InterpolationSet::RestoreVisualInterpolation);
        app.configure_sets(
            FixedPostUpdate,
            InterpolationSet::UpdateVisualInterpolationState,
        );
        app.configure_sets(PostUpdate, InterpolationSet::VisualInterpolation);

        // SYSTEMS
        if P::Components::mode() == ComponentSyncMode::Full {
            app.add_systems(
                PreUpdate,
                restore_from_visual_interpolation::<C>
                    .in_set(InterpolationSet::RestoreVisualInterpolation),
            );
            app.add_systems(
                FixedPostUpdate,
                update_visual_interpolation_status::<C>
                    .in_set(InterpolationSet::UpdateVisualInterpolationState),
            );
            app.add_systems(
                PostUpdate,
                visual_interpolation::<C, P>.in_set(InterpolationSet::VisualInterpolation),
            );
        }
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
        *component.bypass_change_detection() =
            P::Components::lerp(previous_value, current_value, overstep);
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
    mut query: Query<(&mut C, &mut VisualInterpolateStatus<C>)>,
) {
    let kind = C::type_name();
    for (mut component, interpolate_status) in query.iter_mut() {
        if let Some(current_value) = &interpolate_status.current_value {
            trace!(?kind, "Restoring visual interpolation");
            *component.bypass_change_detection() = current_value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;
    use bevy::prelude::*;
    use bevy::utils::Duration;

    use crate::client::sync::SyncConfig;
    use crate::prelude::client::{InterpolationConfig, PredictionConfig};
    use crate::prelude::{LinkConditionerConfig, SharedConfig, TickConfig};
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    use super::*;

    #[derive(Resource, Debug)]
    pub struct Toggle(bool);

    fn setup(tick_duration: Duration, frame_duration: Duration) -> (BevyStepper, Entity) {
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default().disable(false);
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        stepper
            .client_app
            .add_systems(FixedUpdate, fixed_update_increment);
        stepper.client_app.world.insert_resource(Toggle(true));
        stepper
            .client_app
            .add_plugins(VisualInterpolationPlugin::<Component1, MyProtocol>::default());
        let entity = stepper
            .client_app
            .world
            .spawn((
                Component1(0.0),
                VisualInterpolateStatus::<Component1>::default(),
            ))
            .id();
        (stepper, entity)
    }

    fn fixed_update_increment(mut query: Query<&mut Component1>, enabled: Res<Toggle>) {
        if enabled.0 {
            for mut component1 in query.iter_mut() {
                component1.0 += 1.0;
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
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.0
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: None,
                current_value: Some(Component1(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.33,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.66,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(1.0)),
                current_value: Some(Component1(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.66,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            3.00,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(3.0)),
                current_value: Some(Component1(4.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.00,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            4.33,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(4.0)),
                current_value: Some(Component1(5.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
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
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.0
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: None,
                current_value: Some(Component1(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.33,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.66,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(1.0)),
                current_value: Some(Component1(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.66,
            max_relative = 0.1
        );

        stepper.client_app.world.resource_mut::<Toggle>().0 = false;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.00,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.00,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.33,
            max_relative = 0.1
        );
        stepper.client_app.world.resource_mut::<Toggle>().0 = true;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.66,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: Some(Component1(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
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
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            0.0
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: None,
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.75,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: None,
                current_value: Some(Component1(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.5,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.25,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(1.0)),
                current_value: Some(Component1(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.25,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: Some(Component1(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.0,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.75,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: Some(Component1(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
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
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            0.0
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: None,
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.75,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: None,
                current_value: Some(Component1(1.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.5,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            1.25,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(1.0)),
                current_value: Some(Component1(2.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.25,
            max_relative = 0.1
        );

        stepper.client_app.world.resource_mut::<Toggle>().0 = false;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.0,
            max_relative = 0.1
        );

        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.0,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: None,
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.75,
            max_relative = 0.1
        );

        stepper.client_app.world.resource_mut::<Toggle>().0 = true;
        stepper.frame_step();
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<Component1>()
                .unwrap()
                .0,
            2.5,
            max_relative = 0.1
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(entity)
                .get::<VisualInterpolateStatus<Component1>>()
                .unwrap(),
            &VisualInterpolateStatus {
                previous_value: Some(Component1(2.0)),
                current_value: Some(Component1(3.0)),
            }
        );
        assert_relative_eq!(
            stepper
                .client_app
                .world
                .resource::<TimeManager>()
                .overstep(),
            0.5,
            max_relative = 0.1
        );
    }
}
