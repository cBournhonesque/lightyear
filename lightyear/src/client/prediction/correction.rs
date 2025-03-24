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
use bevy::prelude::{Added, Commands, Component, DetectChangesMut, Entity, Query, Res};
use tracing::{debug, trace};

use crate::client::components::SyncComponent;
use crate::client::easings::ease_out_quad;
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

impl<C: Component> Correction<C> {
    /// In case of a TickEvent where the client tick is changed, we need to update the ticks in the buffer
    pub(crate) fn update_ticks(&mut self, delta: i16) {
        self.original_tick = self.original_tick + delta;
        self.final_correction_tick = self.final_correction_tick + delta;
    }
}

/// Perform the correction: we interpolate between the original (incorrect) prediction and the final confirmed value
/// over a period of time. The intermediary state is called the Corrected state.
pub(crate) fn get_corrected_state<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    tick_manager: Res<TickManager>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut C, &mut Correction<C>)>,
) {
    let kind = core::any::type_name::<C>();
    let current_tick = tick_manager.tick();
    for (entity, mut component, mut correction) in query.iter_mut() {
        let mut t = (current_tick - correction.original_tick) as f32
            / (correction.final_correction_tick - correction.original_tick) as f32;
        t = t.clamp(0.0, 1.0);

        // TODO: make the easing configurable
        let t = ease_out_quad(t);
        if t == 1.0 {
            trace!(
                ?t,
                "Correction is over. Removing Correction for: {:?}",
                kind
            );
            commands.entity(entity).remove::<Correction<C>>();
        } else {
            trace!(?t, ?entity, start = ?correction.original_tick, end = ?correction.final_correction_tick, "Applying visual correction for {:?}", kind);
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

/// The flow is:
/// [PreUpdate] C = OriginalC, receive NewC, check_rollback, prepare_rollback: add Correction, set C = NewC, rollback, set C = CorrectedC
/// [PreUpdate] C = CorrectedC
/// [PreUpdate] C = CorrectedC
/// [FixedUpdate] Correction, C = CorrectInterpolatedC
/// i.e. if PreUpdate runs a few times in a row without any FixedUpdate step, the component stays in the CorrectedC state.
/// Instead, right after the rollback, we need to reset the component to the original state
pub(crate) fn set_original_prediction_post_rollback<C: SyncComponent>(
    mut query: Query<(Entity, &mut C, &mut Correction<C>), Added<Correction<C>>>,
) {
    for (entity, mut component, mut correction) in query.iter_mut() {
        // correction has not started (even if a correction happens while a previous correction was going on, current_visual is None)
        if correction.current_visual.is_none() {
            trace!(component = ?core::any::type_name::<C>(), "reset value post-rollback, before first correction");
            // TODO: this is very inefficient.
            //  1. we only do the clone() once but if there's multiple frames before a FixedUpdate, we clone multiple times (mitigated by Added filter)
            //        although Added probably  doesn't work if we have nested Corrections..
            //  2. if there was a FixedUpdate right after the rollback, we wouldn't need to call this at all!
            // If multiple Updates run in a row, we want to show the original_prediction value at the end of the frame,
            // but we also need to keep track of the correct value! We will put it in `correction.current_correction`, since this is what
            // is used to restore the correct value at the start of the next frame
            correction.current_correction = Some(component.clone());
            *component.bypass_change_detection() = correction.original_prediction.clone();
        }
    }
}

/// Before we check for rollbacks and run FixedUpdate, restore the correct component value
pub(crate) fn restore_corrected_state<C: SyncComponent>(
    mut query: Query<(&mut C, &mut Correction<C>)>,
) {
    let kind = core::any::type_name::<C>();
    for (mut component, mut correction) in query.iter_mut() {
        match core::mem::take(&mut correction.current_correction) { Some(correction) => {
            debug!("restoring corrected component: {:?}", kind);
            *component.bypass_change_detection() = correction;
        } _ => {
            debug!(
                "Corrected component was None so couldn't restore: {:?}",
                kind
            );
        }}
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
    use core::time::Duration;

    fn increment_component_system(mut query: Query<(Entity, &mut ComponentCorrection)>) {
        for (entity, mut component) in query.iter_mut() {
            component.0 += 1.0;
        }
    }

    fn setup(
        tick_duration: Duration,
        frame_duration: Duration,
    ) -> (BevyStepper, Entity, Entity, Entity, Entity) {
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
            .client_app
            .add_systems(FixedUpdate, increment_component_system);
        stepper.build();
        stepper.init();
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
        (stepper, confirmed_a, predicted_a, confirmed_b, predicted_b)
    }

    /// Test:
    /// - normal correction
    /// - rollback that happens while correction was under way
    #[test]
    fn test_correction() {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let (mut stepper, confirmed, predicted, _, _) = setup(frame_duration, tick_duration);

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

        // interpolate 20% of the way
        let current_visual = Some(ComponentCorrection(2.0 + ease_out_quad(0.2) * (10.0 - 2.0)));
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
                current_visual,
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
        let current_visual = Some(ComponentCorrection(2.0 + ease_out_quad(0.4) * (11.0 - 2.0)));
        assert_relative_eq!(
            correction.current_visual.as_ref().unwrap().0,
            current_visual.as_ref().unwrap().0
        );
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
        let previous_visual = current_visual.as_ref().unwrap().0;
        assert_relative_eq!(correction.original_prediction.0, previous_visual);
        assert_eq!(correction.original_tick, original_tick);
        assert_eq!(
            correction.final_correction_tick,
            original_tick + (original_tick - rollback_tick)
        );
        // interpolate 20% of the way
        let current_visual = Some(ComponentCorrection(
            previous_visual + ease_out_quad(0.2) * (17.0 - previous_visual),
        ));
        assert_relative_eq!(
            correction.current_visual.as_ref().unwrap().0,
            current_visual.as_ref().unwrap().0
        );
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
        let current_visual = Some(ComponentCorrection(
            previous_visual + ease_out_quad(0.8) * (20.0 - previous_visual),
        ));
        assert_relative_eq!(
            correction.current_visual.as_ref().unwrap().0,
            current_visual.as_ref().unwrap().0
        );
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
        let (mut stepper, confirmed_a, predicted_a, confirmed_b, predicted_b) =
            setup(frame_duration, tick_duration);

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
        // interpolate 20% of the way
        let current_visual = Some(ComponentCorrection(2.0 + ease_out_quad(0.2) * (10.0 - 2.0)));
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
                current_visual,
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

    /// Check that correction still works even if Update runs twice in a row (i.e. we don't have a FixedUpdate on the frame of the rollback)
    #[test]
    fn test_two_consecutive_frame_updates() {
        let frame_duration = Duration::from_millis(10);
        // very long tick duration to guarantee that we have 2 consecutive frames without a tick
        let tick_duration = Duration::from_millis(25);
        let (mut stepper, confirmed, predicted, _, _) = setup(tick_duration, frame_duration);

        // we insert the component at a different frame to not trigger an early rollback
        // (here a rollback isn't triggered because we didn't receive any server packets)
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
        // check that a correction Component is added, however no Correction is applied yet because FixedUpdate didn't run
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
                current_visual: None,
                // the value is 8.0 because multiple frames ran without FixedUpdate running
                current_correction: Some(ComponentCorrection(8.0)),
            }
        );
        // check that the component is still visually the original prediction at the end of the frame
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentCorrection>(predicted),
            Some(&ComponentCorrection(2.0))
        );
        stepper.frame_step();
        // check that this time we ran FixedUpdate, but that we use the Corrected value as the final value to correct towards
        // not the original prediction! If that were the case, we would correct towards ComponentCorrection(3.0)
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Correction<ComponentCorrection>>(predicted)
                .unwrap()
                .current_correction,
            Some(ComponentCorrection(9.0)),
        );
    }
}
