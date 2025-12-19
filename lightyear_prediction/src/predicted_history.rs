//! Managed the history buffer, which is a buffer of the past predicted component states,
//! so that whenever we receive an update from the server we can compare the predicted entity's history with the server update.
use crate::Predicted;
use crate::rollback::DeterministicPredicted;
use bevy_ecs::prelude::*;
use bevy_utils::prelude::DebugName;
use core::ops::Deref;
use lightyear_core::history_buffer::HistoryBuffer;
use lightyear_core::prelude::{LocalTimeline};
use lightyear_core::timeline::SyncEvent;
use lightyear_replication::prelude::{Confirmed, PreSpawned};
use lightyear_sync::prelude::InputTimelineConfig;
#[allow(unused_imports)]
use tracing::{info, trace};

pub type PredictionHistory<C> = HistoryBuffer<C>;

/// If PredictionMode::Full, we store every update on the predicted entity in the PredictionHistory
///
/// This system only handles changes, removals are handled in `apply_component_removal`
pub(crate) fn update_prediction_history<T: Component + Clone>(
    mut query: Query<(Ref<T>, &mut PredictionHistory<T>)>,
    timeline: Res<LocalTimeline>,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = timeline.tick();

    // update history if the predicted component changed
    for (component, mut history) in query.iter_mut() {
        // change detection works even when running the schedule for rollback
        if component.is_changed() {
            // trace!(
            //     "Prediction history changed for tick {tick:?} component {:?}",
            //     DebugName::type_name::<T>()
            // );
            history.add_update(tick, component.deref().clone());
        }
    }
}

/// If there is a TickEvent and the client tick suddenly changes, we need
/// to update the ticks in the history buffer.
///
/// The history buffer ticks are only relevant relative to the current client tick.
/// (i.e. X ticks in the past compared to the current tick)
pub(crate) fn handle_tick_event_prediction_history<C: Component>(
    trigger: On<SyncEvent<InputTimelineConfig>>,
    mut query: Query<&mut PredictionHistory<C>>,
) {
    for mut history in query.iter_mut() {
        trace!(
            "Prediction history updated for {:?} with tick delta {:?}",
            DebugName::type_name::<C>(),
            trigger.tick_delta
        );
        history.update_ticks(trigger.tick_delta);
    }
}

/// If a predicted component is removed on the [`Predicted`] entity, add the removal to the history (for potential rollbacks).
///
/// (if [`Confirmed<C>`] is removed from the component, we don't need to do anything. We might get a rollback
/// by comparing with the history)
pub(crate) fn apply_component_removal_predicted<C: Component>(
    trigger: On<Remove, C>,
    mut predicted_query: Query<&mut PredictionHistory<C>>,
    timeline: Res<LocalTimeline>,
) {
    let tick = timeline.tick();
    // if the component was removed from the Predicted entity, add the Removal to the history
    if let Ok(mut history) = predicted_query.get_mut(trigger.entity) {
        // tick for which we will record the history (either the current client tick or the current rollback tick)
        history.add_remove(tick);
    }
}

/// If a predicted component gets added to [`Predicted`] entity, add a [`PredictionHistory`] component.
///
/// We don't put any value in the history because the `update_history` systems will add the value.
///
/// Predicted: when [`Confirmed<C>`] is added, we potentially do a rollback which will add C
/// PreSpawned:
///   - on the client the component C is added, which should be added to the history
///   - before matching, any rollback should bring us back to the state of C in the history
///   - when Predicted is added (on PreSpawn match), [`Confirmed<C>`] might be added, which shouldn't trigger a rollback
///     because it should match the state of C in the history. We remove PreSpawned to make sure that we rollback to
///     the [`Confirmed<C>`] state
///   - if no match, we also remove PreSpawned, so that the entity is just Predicted (and we rollback to the last [`Confirmed<C>`] state)
pub(crate) fn add_prediction_history<C: Component>(
    trigger: On<
        Add,
        (
            Confirmed<C>,
            C,
            Predicted,
            PreSpawned,
            DeterministicPredicted,
        ),
    >,
    mut commands: Commands,
    // TODO: should we also have With<ShouldBePredicted>?
    query: Query<
        (),
        (
            Without<PredictionHistory<C>>,
            Or<(With<Confirmed<C>>, With<C>)>,
            Or<(
                With<Predicted>,
                With<PreSpawned>,
                With<DeterministicPredicted>,
            )>,
        ),
    >,
) {
    if query.get(trigger.entity).is_ok() {
        trace!(
            "Add prediction history for {:?} on entity {:?}",
            DebugName::type_name::<C>(),
            trigger.entity
        );
        commands
            .entity(trigger.entity)
            .insert(PredictionHistory::<C>::default());
    }
}
