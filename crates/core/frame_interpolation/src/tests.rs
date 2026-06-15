use super::*;
use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::easings::ease_out_quad;
use crate::client::prediction::Predicted;
use crate::client::prediction::rollback::test_utils::received_confirmed_update;
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
    stepper
        .client_app()
        .world_mut()
        .insert_resource(Toggle(true));
    stepper
        .client_app
        .add_plugins(FrameInterpolationPlugin::<InterpolationModeFull>::default());
    let entity = stepper
        .client_app
        .world_mut()
        .spawn((
            InterpolationModeFull(0.0),
            FrameInterpolate::<InterpolationModeFull>::default(),
        ))
        .id();
    stepper.build();
    (stepper, entity)
}

fn fixed_update_increment(
    mut query: Query<&mut InterpolationModeFull>,
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
            .get::<InterpolationModeFull>()
            .unwrap()
            .0,
        1.0
    );
    assert_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: None,
            current_value: Some(InterpolationModeFull(1.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(1.0)),
            current_value: Some(InterpolationModeFull(2.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(3.0)),
            current_value: Some(InterpolationModeFull(4.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(4.0)),
            current_value: Some(InterpolationModeFull(5.0)),
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
            .get::<InterpolationModeFull>()
            .unwrap()
            .0,
        1.0
    );
    assert_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: None,
            current_value: Some(InterpolationModeFull(1.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(1.0)),
            current_value: Some(InterpolationModeFull(2.0)),
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

    stepper.client_app().world_mut().resource_mut::<Toggle>().0 = false;
    stepper.frame_step();
    assert_relative_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
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
    stepper.client_app().world_mut().resource_mut::<Toggle>().0 = true;
    stepper.frame_step();
    assert_relative_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
            current_value: Some(InterpolationModeFull(3.0)),
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
            .get::<InterpolationModeFull>()
            .unwrap()
            .0,
        0.0
    );
    assert_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: None,
            current_value: Some(InterpolationModeFull(1.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(1.0)),
            current_value: Some(InterpolationModeFull(2.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
            current_value: Some(InterpolationModeFull(3.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
            current_value: Some(InterpolationModeFull(3.0)),
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
            .get::<InterpolationModeFull>()
            .unwrap()
            .0,
        0.0
    );
    assert_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: None,
            current_value: Some(InterpolationModeFull(1.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(1.0)),
            current_value: Some(InterpolationModeFull(2.0)),
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

    stepper.client_app().world_mut().resource_mut::<Toggle>().0 = false;
    stepper.frame_step();
    assert_relative_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
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
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
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

    stepper.client_app().world_mut().resource_mut::<Toggle>().0 = true;
    stepper.frame_step();
    assert_relative_eq!(
        stepper
            .client_app
            .world()
            .entity(entity)
            .get::<InterpolationModeFull>()
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
            .get::<FrameInterpolate<InterpolationModeFull>>()
            .unwrap(),
        &FrameInterpolate {
            trigger_change_detection: false,
            previous_value: Some(InterpolationModeFull(2.0)),
            current_value: Some(InterpolationModeFull(3.0)),
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
    stepper
        .client_app()
        .world_mut()
        .insert_resource(Toggle(true));
    stepper
        .client_app
        .add_systems(FixedUpdate, fixed_update_increment);
    stepper
        .client_app
        .add_plugins(FrameInterpolationPlugin::<ComponentCorrection>::default());
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
            FrameInterpolate::<ComponentCorrection>::default(),
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
            .get::<FrameInterpolate<ComponentCorrection>>()
            .unwrap(),
        &FrameInterpolate::<ComponentCorrection> {
            trigger_change_detection: false,
            // TODO: maybe we'd like to interpolate from 1.0 here? we could have custom logic where
            //  post-rollback if previous_value is None and Correction is enabled, we set previous_value to original_prediction?
            previous_value: None,
            current_value: current_visual,
        }
    );
}
