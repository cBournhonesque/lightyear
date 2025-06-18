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
//! - PreUpdate: we see that there is a rollback needed. We insert
//!   Correction { original_value = PT, start_tick, end_tick }
//! - RunRollback, which lets us compute the correct CT value.
//! - FixedUpdate: we run the simulation to get the new value C(T+1)
//! - FixedPostUpdate: set the component value to the interpolation between PT and C(T+1)
//!
//! - PreUpdate: restore the C(T+1) value (corrected value at the current tick T+1)
//!   - if there is a rollback, restart correction from the current corrected value
//! - FixedUpdate: run the simulation to compute C(T+2).
//! - FixedPostUpdate: set the component value to the interpolation between PT (predicted value at rollback start T) and C(T+2)
use crate::manager::PredictionManager;
use crate::registry::PredictionRegistry;
use crate::SyncComponent;
use bevy::prelude::*;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_utils::easings::ease_out_quad;

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
    prediction_registry: Res<PredictionRegistry>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut C, &mut Correction<C>)>,
    timeline: Single<&LocalTimeline, With<PredictionManager>>,
) {
    let kind = core::any::type_name::<C>();
    let current_tick = timeline.tick();
    for (entity, mut component, mut correction) in query.iter_mut() {
        let mut t = (current_tick - correction.original_tick) as f32
            / (correction.final_correction_tick - correction.original_tick) as f32;
        t = t.clamp(0.0, 1.0);

        // TODO: make the easing configurable
        let t = ease_out_quad(t);
        if t == 1.0 {
            trace!(
                ?t,
                "Correction is over. Removing Correction for: {:?}", kind
            );
            commands.entity(entity).remove::<Correction<C>>();
        } else {
            trace!(?t, ?entity, start = ?correction.original_tick, end = ?correction.final_correction_tick, "Applying visual correction for {:?}", kind);
            // store the current corrected value so that we can restore it at the start of the next frame
            correction.current_correction = Some(component.clone());
            // TODO: avoid all these clones
            // visually update the component
            let visual = prediction_registry.correct(
                correction.original_prediction.clone(),
                component.clone(),
                t,
            );
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
            trace!(component = ?core::any::type_name::<C>(), "Set component to original non-corrected prediction");
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
        if let Some(correction) = core::mem::take(&mut correction.current_correction) {
            trace!(
                "Restoring corrected component before FixedUpdate: {:?}",
                kind
            );
            *component.bypass_change_detection() = correction;
        }
    }
}
