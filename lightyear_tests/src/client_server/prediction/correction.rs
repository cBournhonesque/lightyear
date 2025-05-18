use crate::client_server::prediction::{trigger_rollback_check, trigger_rollback_system, RollbackInfo};
use crate::protocol::CompCorr;
use crate::stepper::ClientServerStepper;
use approx::assert_relative_eq;
use bevy::prelude::*;
use core::time::Duration;
use lightyear_prediction::correction::Correction;
use lightyear_prediction::plugin::PredictionSet;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::Predicted;
use lightyear_replication::components::Confirmed;
use lightyear_replication::prelude::ReplicationSet;
use lightyear_utils::easings::ease_out_quad;
use test_log::test;

fn increment_component_system(mut query: Query<&mut CompCorr>) {
        for mut component in query.iter_mut() {
            component.0 += 1.0;
        }
    }

    fn setup(
        tick_duration: Duration,
        frame_duration: Duration,
    ) -> (ClientServerStepper, Entity, Entity, Entity, Entity) {
        let mut stepper = ClientServerStepper::new(tick_duration, frame_duration);

        stepper.new_client();
        stepper
            .client_app()
            .add_systems(FixedUpdate, increment_component_system);
        stepper.client_app().add_event::<RollbackInfo>();
        stepper.client_app().add_systems(PreUpdate, trigger_rollback_system.after(ReplicationSet::Receive).before(PredictionSet::CheckRollback));
        stepper.init();
        let tick = stepper.client_tick(0);

        let predicted_a = stepper
            .client_app()
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: None,
            })
            .id();
        let confirmed_a = stepper
            .client_app()
            .world_mut()
            .spawn((
                Confirmed {
                    tick,
                    predicted: Some(predicted_a),
                    ..Default::default()
                },
                CompCorr(2.0),
            ))
            .id();
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted_a)
            .get_mut::<Predicted>()
            .unwrap()
            .confirmed_entity = Some(confirmed_a);
        let predicted_b = stepper
            .client_app()
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: None,
            })
            .id();
        let confirmed_b = stepper
            .client_app()
            .world_mut()
            .spawn((
                Confirmed {
                    tick,
                    predicted: Some(predicted_b),
                    ..Default::default()
                },
                CompCorr(2.0),
            ))
            .id();
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted_b)
            .get_mut::<Predicted>()
            .unwrap()
            .confirmed_entity = Some(confirmed_b);

        stepper.frame_step(1);
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
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .insert(CompCorr(1.0));
        stepper.frame_step(1);

        // trigger a rollback (the predicted value doesn't exist in the prediction history)
        let original_tick = stepper.client_tick(0);
        let rollback_tick = original_tick - 5;
        info!(?rollback_tick, "history {:?}", stepper.client_app().world().get::<PredictionHistory<CompCorr>>(predicted));
        trigger_rollback_check(&mut stepper, rollback_tick);

        stepper.frame_step(1);
        // check that a correction is applied

        // interpolate 20% of the way
        let current_visual = Some(CompCorr(2.0 + ease_out_quad(0.2) * (10.0 - 2.0)));
        assert_eq!(
            stepper
                .client_app()
                .world()
                .get::<Correction<CompCorr>>(predicted)
                .unwrap(),
            &Correction::<CompCorr> {
                original_prediction: CompCorr(2.0),
                original_tick,
                final_correction_tick: original_tick + (original_tick - rollback_tick),
                current_visual,
                current_correction: Some(CompCorr(10.0)),
            }
        );

        // check that the correction value has been incremented and that the visual value has been updated correctly
        stepper.frame_step(1);
        let correction = stepper
            .client_app()
            .world()
            .get::<Correction<CompCorr>>(predicted)
            .unwrap();
        let current_visual = Some(CompCorr(2.0 + ease_out_quad(0.4) * (11.0 - 2.0)));
        assert_relative_eq!(
            correction.current_visual.as_ref().unwrap().0,
            current_visual.as_ref().unwrap().0
        );
        assert_eq!(correction.current_correction.as_ref().unwrap().0, 11.0);

        // trigger a new rollback while the correction is under way
        let original_tick = stepper.client_tick(0);
        let rollback_tick = original_tick - 5;
        trigger_rollback_check(&mut stepper, rollback_tick);
        stepper.frame_step(1);

        // check that the correction has been updated
        let correction = stepper
            .client_app()
            .world()
            .get::<Correction<CompCorr>>(predicted)
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
        let current_visual = Some(CompCorr(
            previous_visual + ease_out_quad(0.2) * (17.0 - previous_visual),
        ));
        assert_relative_eq!(
            correction.current_visual.as_ref().unwrap().0,
            current_visual.as_ref().unwrap().0
        );
        assert_eq!(correction.current_correction.as_ref().unwrap().0, 17.0);
        stepper.frame_step(3);
        let correction = stepper
            .client_app()
            .world()
            .get::<Correction<CompCorr>>(predicted)
            .unwrap();
        // interpolate 80% of the way
        let current_visual = Some(CompCorr(
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
            .client_app()
            .world_mut()
            .entity_mut(predicted_a)
            .insert(CompCorr(1.0));
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted_b)
            .insert(CompCorr(1.0));
        stepper.frame_step(1);

        // trigger a rollback (the predicted value doesn't exist in the prediction history)
        let original_tick = stepper.client_tick(0);
        let rollback_tick = original_tick - 5;
        // add a history with the correct value for entity b to make sure that it is correctly predicted
        let mut history = PredictionHistory::<CompCorr>::default();
        history.add_update(rollback_tick, CompCorr(4.0));
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted_b)
            .insert(history);
         trigger_rollback_check(&mut stepper, rollback_tick);

        stepper.frame_step(1);
        // check that a correction is applied
        // interpolate 20% of the way
        let current_visual = Some(CompCorr(2.0 + ease_out_quad(0.2) * (10.0 - 2.0)));
        assert_eq!(
            stepper
                .client_app()
                .world()
                .get::<Correction<CompCorr>>(predicted_a)
                .unwrap(),
            &Correction::<CompCorr> {
                original_prediction: CompCorr(2.0),
                original_tick,
                final_correction_tick: original_tick + (original_tick - rollback_tick),
                current_visual,
                current_correction: Some(CompCorr(10.0)),
            }
        );
        // check that no correction is applied for entities that are correctly predicted
        assert!(
            stepper
                .client_app()
                .world()
                .get::<Correction<CompCorr>>(predicted_b)
                .is_none()
        );
    }

    /// Check that correction still works even if Update runs twice in a row (i.e. we don't have a FixedUpdate on the frame of the rollback)
    #[test]
    fn test_two_consecutive_frame_updates() {
        let frame_duration = Duration::from_millis(10);
        // very long tick duration to guarantee that we have 2 consecutive frames without a tick
        // choose a value where Syncing stops at the end of tick
        let tick_duration = Duration::from_millis(21);
        let (mut stepper, confirmed, predicted, _, _) = setup(tick_duration, frame_duration);

        // we insert the component at a different frame to not trigger an early rollback
        // (here a rollback isn't triggered because we didn't receive any server packets)
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .insert(CompCorr(1.0));
        stepper.frame_step(1);

        // trigger a rollback (the predicted value doesn't exist in the prediction history)
        let original_tick = stepper.client_tick(0);
        let rollback_tick = original_tick - 5;
         trigger_rollback_check(&mut stepper, rollback_tick);
        stepper.frame_step(1);
        // check that a correction Component is added, however no Correction is applied yet because FixedUpdate didn't run
        assert_eq!(
            stepper
                .client_app()
                .world()
                .get::<Correction<CompCorr>>(predicted)
                .unwrap(),
            &Correction::<CompCorr> {
                original_prediction: CompCorr(2.0),
                original_tick,
                final_correction_tick: original_tick + (original_tick - rollback_tick),
                current_visual: None,
                // the value is 8.0 because multiple frames ran without FixedUpdate running
                current_correction: Some(CompCorr(8.0)),
            }
        );
        // check that the component is still visually the original prediction at the end of the frame
        assert_eq!(
            stepper
                .client_app()
                .world()
                .get::<CompCorr>(predicted),
            Some(&CompCorr(2.0))
        );
        // progress enough so that we reach the next tick
        stepper.frame_step(2);
        // check that this time we ran FixedUpdate, but that we use the Corrected value as the final value to correct towards
        // not the original prediction! If that were the case, we would correct towards CompCorr(3.0)
        assert_eq!(
            stepper
                .client_app()
                .world()
                .get::<Correction<CompCorr>>(predicted)
                .unwrap()
                .current_correction,
            Some(CompCorr(9.0)),
        );
    }