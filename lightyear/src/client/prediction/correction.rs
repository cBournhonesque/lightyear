//! This module provides the ability to smooth the rollback (from the Predicted state to the Corrected state) over a certain amount of ticks, instead
//! of just snapping back instantly to the Corrected state

// maybe multiple correction_modes:
// - instant (default)
// - interpolate (provided)
// - custom

use bevy::prelude::{Commands, Component, DetectChangesMut, Entity, Query, Res};
use tracing::debug;

use crate::_reexport::ComponentProtocol;
use crate::client::components::{LerpFn, SyncComponent, SyncMetadata};
use crate::client::easings::ease_out_quad;
use crate::prelude::{Tick, TickManager};
use crate::protocol::Protocol;

// TODO: instead of requiring the component to implement the correction, we could have a separate
//  'type registry' that stores the correction function for each component type.
//  or we register in the protocol (that is user defined), the correction function for each component type
//  P::CorrectionFn<C> -> CorrectionFn<C>
//  or something like P::Interpolate<C: ComponentKind> -> InterpolationFn<C>

// pub trait CorrectionFn<C> {
//     /// How do we perform the correction between the original Predicted state and the Corrected state?
//     /// (t is the interpolation factor between the tick of the original Predicted state and the tick of the Corrected state)
//     fn correct(predicted: C, corrected: C, t: f32) -> C;
//
//     // fn get_correction_final_tick(&self, prediction_tick: Tick) -> Tick;
// }

/// We snapback instantly to the Corrected state
pub struct InstantCorrector;
impl<C: Clone> LerpFn<C> for InstantCorrector {
    fn lerp(predicted: &C, corrected: &C, t: f32) -> C {
        // the correction is instant, so we just return the Corrected state
        corrected.clone()
    }

    // fn get_correction_final_tick(&self, prediction_tick: Tick) -> Tick {
    //     // the correction is instant, so the final tick is the same as the prediction tick
    //     prediction_tick
    // }
}

/// We use the components interpolation behaviour to interpolate from the Predicted state to the
/// Corrected state
pub struct InterpolatedCorrector;
// {
//     The number of ticks that we will perform the correction over
// correction_ticks: Tick,
// }

// impl<C: InterpolatedComponent<C>> CorrectionFn<C> for InterpolatedCorrector {
//     fn correct(predicted: C, corrected: C, t: f32) -> C {
//         C::lerp(predicted, corrected, t)
//     }
//     //
//     // fn get_correction_final_tick(&self, prediction_tick: Tick) -> Tick {
//     //     prediction_tick + self.correction_ticks
//     // }
// }

#[derive(Component, Debug)]
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
    /// and the final correction value
    pub current_correction: Option<C>,
}

/// Visually update the component to the a value that is interpolated between the original prediction
/// and the Corrected state
pub(crate) fn get_visually_corrected_state<C: SyncComponent, P: Protocol>(
    tick_manager: Res<TickManager>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut C, &mut Correction<C>)>,
) where
    P::Components: SyncMetadata<C>,
{
    for (entity, mut component, mut correction) in query.iter_mut() {
        let current_tick = tick_manager.tick();
        let mut t = (current_tick - correction.original_tick) as f32
            / (correction.final_correction_tick - correction.original_tick) as f32;
        t = t.clamp(0.0, 1.0);
        let t = ease_out_quad(t);
        if t == 1.0 || &correction.original_prediction == component.as_ref() {
            debug!(
                ?t,
                "Correction is over. Removing Correction for: {:?}",
                component.name()
            );
            // correction is over
            commands.entity(entity).remove::<Correction<C>>();
        } else {
            debug!(?t, ?entity, start = ?correction.original_tick, end = ?correction.final_correction_tick, "Applying visual correction for {:?}", component.name());
            // store the current corrected value so that we can restore it at the start of the next frame
            correction.current_correction = Some(component.clone());
            // TODO: avoid all these clones
            // visually update the component
            let visual =
                P::Components::correct(&correction.original_prediction, component.as_ref(), t);
            // store the current visual value
            correction.current_visual = Some(visual.clone());
            // set the component value to the visual value
            *component.bypass_change_detection() = visual;
        }
    }
}

/// At the start of the next frame, restore
pub(crate) fn restore_corrected_state<C: SyncComponent>(
    mut query: Query<(&mut C, &mut Correction<C>)>,
) {
    for (mut component, mut correction) in query.iter_mut() {
        if let Some(correction) = std::mem::take(&mut correction.current_correction) {
            debug!("restoring corrected component: {:?}", component.name());
            *component.bypass_change_detection() = correction;
        } else {
            debug!(
                "Corrected component was None so couldn't restore: {:?}",
                component.name()
            );
        }
    }
}

// - on rollback, we store the original Predicted position and the current tick
// - we compute the final_correction_tick = current_tick + correction_ticks
// - during rollback, the Predicted entity will take the Corrected position.
// - in PostUpdate, during the correction_ticks, we will interpolated between the old
