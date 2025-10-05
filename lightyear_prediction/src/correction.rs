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

use crate::SyncComponent;
use crate::manager::PredictionManager;
use crate::predicted_history::PredictionHistory;
use crate::registry::PredictionRegistry;
use crate::rollback::RollbackSet;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationSet};
use lightyear_interpolation::prelude::InterpolationRegistry;
use lightyear_replication::delta::Diffable;
use tracing::trace;

/// The visual value of the component before the rollback started
#[derive(Component, Debug, Reflect)]
pub struct PreviousVisual<C: Component>(pub C);

#[derive(Component, Debug, Reflect)]
pub struct VisualCorrection<D> {
    /// The error between the original visual value and the new visual value.
    /// Will decay over time.
    pub error: D,
}

pub fn add_correction_systems<
    C: SyncComponent + Diffable<D>,
    D: Default + Clone + Debug + Send + Sync + 'static,
>(
    app: &mut App,
) {
    // When rollback finishes, compute the new corrected visual value and compare it with the original visual value
    // to set the visual correction error.
    app.add_systems(
        PreUpdate,
        update_frame_interpolation_post_rollback::<C, D>.in_set(RollbackSet::EndRollback),
    );
    app.configure_sets(
        PostUpdate,
        // If FrameInterpolation runs after Correction, it would overwrite the applied correction.
        RollbackSet::VisualCorrection.after(FrameInterpolationSet::Interpolate),
    );
    app.add_systems(
        PostUpdate,
        add_visual_correction::<C, D>.in_set(RollbackSet::VisualCorrection),
    );
}

/// After the rollback is over, we need to update the values in the [`FrameInterpolate<C>`] component.
/// This is important to run now and not in FixedUpdate because FixedUpdate could not run this frame.
/// (if we have two frames in a row)
///
/// If we have correction enabled, then we can compute the error between the previous visual value
/// [`PreviousVisual<C>`] and the new visual value.
pub(crate) fn update_frame_interpolation_post_rollback<
    C: SyncComponent + Diffable<D>,
    D: Debug + Send + Sync + 'static,
>(
    time: Res<Time<Fixed>>,
    // only run if there is a VisualCorrection<C> to do.
    timeline: Single<&LocalTimeline, With<PredictionManager>>,
    registry: Res<InterpolationRegistry>,
    mut query: Query<(
        Entity,
        &C,
        Option<&PreviousVisual<C>>,
        &PredictionHistory<C>,
        &mut FrameInterpolate<C>,
    )>,
    mut commands: Commands,
) {
    // NOTE: this is the overstep from the previous frame since we are running this before RunFixedMainLoop
    let overstep = time.overstep_fraction();
    let tick = timeline.tick();
    for (entity, component, previous_visual, history, mut interpolate) in query.iter_mut() {
        // update the FrameInterpolation with the last 2 history values
        interpolate.current_value = Some(component.clone());
        interpolate.previous_value = history.second_most_recent(tick).cloned();
        let Some(previous) = &interpolate.previous_value else {
            continue;
        };

        // compute the new visual value post-rollback but interpolating between the last 2 states of the history
        if let Some(previous_visual) = previous_visual {
            let current_visual = registry.interpolate(previous.clone(), component.clone(), overstep);
            // error = previous_visual - current_visual
            let error = current_visual.diff(&previous_visual.0);
            trace!(
                ?tick,
                ?entity,
                ?current_visual,
                ?previous_visual,
                ?error,
                // two_previous_values = ?interpolate,
                // ?history,
                "Updating VisualCorrection post rollback for {:?}",
                DebugName::type_name::<C>()
            );
            commands
                .entity(entity)
                .insert(VisualCorrection::<D> { error })
                .remove::<PreviousVisual<C>>();
        }
    }
}

/// Add the visual correction error to the visual component, and
/// decay the visual correction error over time.
///
/// If it gets small enough, we remove the `VisualCorrection<C>` component.
///
/// The delta D must have a interpolation function registered in the [`InterpolationRegistry`].
pub(crate) fn add_visual_correction<
    C: SyncComponent + Diffable<D>,
    D: Default + Clone + Debug + Send + Sync + 'static,
>(
    time: Res<Time<Virtual>>,
    prediction: Res<PredictionRegistry>,
    manager: Single<&PredictionManager>,
    mut query: Query<(Entity, &mut C, &mut VisualCorrection<D>)>,
    mut commands: Commands,
) {
    let r = manager.correction_policy.lerp_ratio(time.delta());
    query
        .iter_mut()
        .for_each(|(entity, mut component, mut visual_correction)| {
            let mut error_as_transform = C::base_value();
            error_as_transform.apply_diff(&visual_correction.error);
            if !prediction.should_rollback(&C::base_value(), &error_as_transform) {
                trace!(
                    ?visual_correction,
                    "Removing visual correction error {:?} since it is already small enough",
                    DebugName::type_name::<C>()
                );
                commands.entity(entity).remove::<VisualCorrection<D>>();
                return;
            }
            let previous_error = &visual_correction.error;
            let new_error = prediction
                .apply_correction::<C, D>(previous_error.clone(), r)
                .expect("No correction fn was found");
            component.bypass_change_detection().apply_diff(&new_error);
            trace!(
                ?entity,
                ?component,
                ?previous_error,
                ?new_error,
                ?r,
                "Applied visual correction and decaying error for {:?}",
                DebugName::type_name::<C>()
            );
            visual_correction.error = new_error;
        });
}

#[derive(Component, Debug, Reflect)]
pub struct CorrectionPolicy {
    /// Period of time to decay the error by `decay_ratio`
    decay_period: core::time::Duration,
    /// Fraction of the error remaining after `decay_period` has passed.
    ///
    /// For example if `decay_period` is 1 second and `decay_ratio` is 0.3, then only 30% of the original error
    /// remains after 1 second.
    decay_ratio: f32,
    /// We will stop applying correction after this amount of time has passed since the rollback started.
    max_correction_period: core::time::Duration,
}

impl Default for CorrectionPolicy {
    fn default() -> Self {
        Self {
            decay_period: core::time::Duration::from_millis(100),
            decay_ratio: 0.5,
            max_correction_period: core::time::Duration::from_secs(500),
        }
    }
}

impl CorrectionPolicy {
    /// Returns the lerp constant to use for exponentially decaying the error in a framestep-insensitive way
    ///
    /// See: <https://www.youtube.com/watch?v=LSNQuFEDOyQ>
    #[inline]
    pub fn lerp_ratio(&self, delta: core::time::Duration) -> f32 {
        let dt = delta.as_secs_f32();
        let neg_decay_constant = self.decay_ratio.ln() / self.decay_period.as_secs_f32();
        (neg_decay_constant * dt).exp()
    }

    pub fn instant_correction() -> Self {
        Self {
            decay_period: core::time::Duration::from_millis(1),
            decay_ratio: 0.0000001,
            max_correction_period: core::time::Duration::from_millis(10),
        }
    }
}
