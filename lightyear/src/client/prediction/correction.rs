//! This module provides the ability to smooth the rollback (from the Predicted state to the Corrected state) over a period
//! of time instead of just snapping back instantly to the Corrected state. This might help hide rollback artifacts.
//! We will call the interpolated state the Visual state.
//!
//! For example the current tick is 10, and you have a predicted value P10.
//! You receive a confirmed update C5 for tick 5, which doesn't match with the value we had stored in the
//! prediction history at tick 5. This means we need to rollback.
//!
//! Without correction, we would simply snap back the value at tick 5 to C5, and then re-run the simulation
//! from tick 5 to 10 to get a new value C10 at tick 10. The simulation will visually snap back from (predicted) P10 to (corrected) C10.
//! Instead what we can do is correct the value from P10 to C10 over a period of time.
//!
//! The flow is (if T is the tick for the start of the rollback, and X is the current tick)
//! - PreUpdate: we see that there is a rollback needed. We insert Correction {
//!   original_value = PT, start_tick, end_tick
//! }
//! - RunRollback, which lets us compute the correct CT value.
//! - FixedUpdate: we run the simulation to get the new value C(T+1)
//! - FixedPostUpdate: set the component value to the interpolation between PT and C(T+1)
//!
//! - PreUpdate: restore the C(T+1) value (corrected value at the current tick T+1)
//!   - if there is a rollback, restart correction from the current corrected value
//! - FixedUpdate: run the simulation to compute C(T+2).
//! - FixedPostUpdate: set the component value to the interpolation between PT (predicted value at rollback start T) and C(T+2)
use bevy::prelude::{Commands, Component, DetectChangesMut, Entity, Query, Res};
use tracing::debug;

use crate::client::components::SyncComponent;
use crate::prelude::{ComponentRegistry, Tick, TickManager};

#[derive(Component, Debug, PartialEq)]
pub struct Correction<C: Component> {
    /// This is what the original predicted value was before any correction was applied
    pub original_prediction: C,
    /// This is the tick at which we started the correction (i.e. where we found that a rollback was necessary)
    pub original_tick: Tick,
    /// This is the tick at which we will have finished the correction
    pub final_correction_tick: Tick,
    /// This is the current visual value. We compute this so that if we rollback again in the middle of an
    /// existing correction, we start again from the current visual value.
    pub current_visual: Option<C>,
    /// This is the current objective (corrected) value. We need this to swap between the visual correction
    /// (interpolated between the original prediction and the final correction)
    /// and the final correction value (the correct value that we are simulating)
    pub current_correction: Option<C>,
}

/// Perform the correction: we interpolate between the original (incorrect) prediction and the final confirmed value
/// over a period of time. The intermediary state is called the Corrected state.
pub(crate) fn get_corrected_state<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    tick_manager: Res<TickManager>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut C, &mut Correction<C>)>,
) {
    let kind = std::any::type_name::<C>();
    let current_tick = tick_manager.tick();
    for (entity, mut component, mut correction) in query.iter_mut() {
        let mut t = (current_tick - correction.original_tick) as f32
            / (correction.final_correction_tick - correction.original_tick) as f32;
        t = t.clamp(0.0, 1.0);

        // TODO: make the easing configurable
        //  let t = ease_out_quad(t);
        if t == 1.0 || &correction.original_prediction == component.as_ref() {
            debug!(
                ?t,
                "Correction is over. Removing Correction for: {:?}", kind
            );
            commands.entity(entity).remove::<Correction<C>>();
        } else {
            debug!(?t, ?entity, start = ?correction.original_tick, end = ?correction.final_correction_tick, "Applying visual correction for {:?}", kind);
            // store the current corrected value so that we can restore it at the start of the next frame
            correction.current_correction = Some(component.clone());
            // TODO: avoid all these clones
            // visually update the component
            let visual =
                component_registry.correct(&correction.original_prediction, component.as_ref(), t);
            // store the current visual value
            correction.current_visual = Some(visual.clone());
            // set the component value to the visual value
            *component.bypass_change_detection() = visual;
        }
    }
}

/// Before we check for rollbacks and run FixedUpdate, restore the correct component value
pub(crate) fn restore_corrected_state<C: SyncComponent>(
    mut query: Query<(&mut C, &mut Correction<C>)>,
) {
    let kind = std::any::type_name::<C>();
    for (mut component, mut correction) in query.iter_mut() {
        if let Some(correction) = std::mem::take(&mut correction.current_correction) {
            debug!("restoring corrected component: {:?}", kind);
            *component.bypass_change_detection() = correction;
        } else {
            debug!(
                "Corrected component was None so couldn't restore: {:?}",
                kind
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::components::Confirmed;
    use crate::client::config::ClientConfig;
    use crate::client::prediction::predicted_history::PredictionHistory;
    use crate::client::prediction::rollback::test_utils::received_confirmed_update;
    use crate::client::prediction::Predicted;
    use crate::prelude::client::PredictionConfig;
    use crate::prelude::{SharedConfig, TickConfig};
    use crate::tests::protocol::ComponentCorrection;
    use crate::tests::stepper::BevyStepper;
    use approx::assert_relative_eq;
    use bevy::app::FixedUpdate;
    use bevy::prelude::default;
    use std::time::Duration;

    fn increment_component_system(mut query: Query<(Entity, &mut ComponentCorrection)>) {
        for (entity, mut component) in query.iter_mut() {
            component.0 += 1.0;
        }
    }

    /// Test:
    /// - normal correction
    /// - rollback that happens while correction was under way
    #[test]
    fn test_correction() {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
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
        let mut stepper = BevyStepper::new(shared_config, client_config, frame_duration);
        stepper
            .client_app
            .add_systems(FixedUpdate, increment_component_system);
        stepper.build();
        stepper.init();

        // add predicted/confirmed entities
        let tick = stepper.client_tick();
        let confirmed = stepper
            .client_app
            .world_mut()
            .spawn((
                Confirmed {
                    tick,
                    ..Default::default()
                },
                ComponentCorrection(2.0),
            ))
            .id();
        let predicted = stepper
            .client_app
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: Some(confirmed),
            })
            .id();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .get_mut::<Confirmed>()
            .unwrap()
            .predicted = Some(predicted);
        stepper.frame_step();

        // we insert the component at a different frame to not trigger an early rollback
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentCorrection(1.0));
        stepper.frame_step();

        // trigger a rollback (the predicted value doesn't exist in the prediction history)
        let original_tick = stepper.client_tick();
        let rollback_tick = original_tick - 5;
        received_confirmed_update(&mut stepper, confirmed, rollback_tick);

        stepper.frame_step();
        // check that a correction is applied
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Correction<ComponentCorrection>>(predicted)
                .unwrap(),
            &Correction::<ComponentCorrection> {
                original_prediction: ComponentCorrection(2.0),
                original_tick,
                final_correction_tick: original_tick + (original_tick - rollback_tick),
                // interpolate 20% of the way
                current_visual: Some(ComponentCorrection(3.6)),
                current_correction: Some(ComponentCorrection(10.0)),
            }
        );

        // check that the correction value has been incremented and that the visual value has been updated correctly
        stepper.frame_step();
        let correction = stepper
            .client_app
            .world()
            .get::<Correction<ComponentCorrection>>(predicted)
            .unwrap();
        assert_relative_eq!(correction.current_visual.as_ref().unwrap().0, 5.6);
        assert_eq!(correction.current_correction.as_ref().unwrap().0, 11.0);

        // trigger a new rollback while the correction is under way
        let original_tick = stepper.client_tick();
        let rollback_tick = original_tick - 5;
        received_confirmed_update(&mut stepper, confirmed, rollback_tick);
        stepper.frame_step();

        // check that the correction has been updated
        let correction = stepper
            .client_app
            .world()
            .get::<Correction<ComponentCorrection>>(predicted)
            .unwrap();
        // the new correction starts from the previous visual value
        assert_relative_eq!(correction.original_prediction.0, 5.6);
        assert_eq!(correction.original_tick, original_tick);
        assert_eq!(
            correction.final_correction_tick,
            original_tick + (original_tick - rollback_tick)
        );
        // interpolate 20% of the way
        assert_relative_eq!(correction.current_visual.as_ref().unwrap().0, 7.88);
        assert_eq!(correction.current_correction.as_ref().unwrap().0, 17.0);
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();
        let correction = stepper
            .client_app
            .world()
            .get::<Correction<ComponentCorrection>>(predicted)
            .unwrap();
        // interpolate 80% of the way
        assert_relative_eq!(correction.current_visual.as_ref().unwrap().0, 17.12);
        assert_eq!(correction.current_correction.as_ref().unwrap().0, 20.0);
    }

    /// Test that if:
    /// - entity A gets mispredicted
    /// - entity B is correctly predicted
    /// then:
    /// - a rollback happens, but we only add correction for entity A
    #[test]
    fn test_no_correction_if_no_misprediction() {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
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
        let mut stepper = BevyStepper::new(shared_config, client_config, frame_duration);
        stepper
            .client_app
            .add_systems(FixedUpdate, increment_component_system);
        stepper.build();
        stepper.init();

        // add predicted/confirmed entities
        let tick = stepper.client_tick();
        let confirmed_a = stepper
            .client_app
            .world_mut()
            .spawn((
                Confirmed {
                    tick,
                    ..Default::default()
                },
                ComponentCorrection(2.0),
            ))
            .id();
        let predicted_a = stepper
            .client_app
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: Some(confirmed_a),
            })
            .id();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed_a)
            .get_mut::<Confirmed>()
            .unwrap()
            .predicted = Some(predicted_a);
        let confirmed_b = stepper
            .client_app
            .world_mut()
            .spawn((
                Confirmed {
                    tick,
                    ..Default::default()
                },
                ComponentCorrection(2.0),
            ))
            .id();
        let predicted_b = stepper
            .client_app
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: Some(confirmed_a),
            })
            .id();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed_b)
            .get_mut::<Confirmed>()
            .unwrap()
            .predicted = Some(predicted_b);
        stepper.frame_step();

        // we insert the component at a different frame to not trigger an early rollback
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_a)
            .insert(ComponentCorrection(1.0));
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_b)
            .insert(ComponentCorrection(1.0));
        stepper.frame_step();

        // trigger a rollback (the predicted value doesn't exist in the prediction history)
        let original_tick = stepper.client_tick();
        let rollback_tick = original_tick - 5;
        // add a history with the correct value for entity b to make sure that it is correctly predicted
        let mut history = PredictionHistory::<ComponentCorrection>::default();
        history.add_update(rollback_tick, ComponentCorrection(4.0));
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_b)
            .insert(history);
        received_confirmed_update(&mut stepper, confirmed_a, rollback_tick);

        stepper.frame_step();
        // check that a correction is applied
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Correction<ComponentCorrection>>(predicted_a)
                .unwrap(),
            &Correction::<ComponentCorrection> {
                original_prediction: ComponentCorrection(2.0),
                original_tick,
                final_correction_tick: original_tick + (original_tick - rollback_tick),
                // interpolate 20% of the way
                current_visual: Some(ComponentCorrection(3.6)),
                current_correction: Some(ComponentCorrection(10.0)),
            }
        );
        // check that no correction is applied for entities that are correctly predicted
        assert!(stepper
            .client_app
            .world()
            .get::<Correction<ComponentCorrection>>(predicted_b)
            .is_none());
    }
}
