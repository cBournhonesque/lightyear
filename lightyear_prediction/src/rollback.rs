use super::predicted_history::PredictionHistory;
use super::resource_history::ResourceHistory;
use super::{Predicted, PredictionMode, SyncComponent};
use alloc::vec::Vec;
use crate::correction::PreviousVisual;
use crate::despawn::PredictionDisable;
use crate::diagnostics::PredictionMetrics;
use crate::manager::{LastConfirmedInput, PredictionManager, RollbackMode};
use crate::plugin::PredictionSet;
use crate::prespawn::PreSpawned;
use crate::registry::PredictionRegistry;
use bevy_app::{App, FixedMain, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::ScheduleLabel;
use bevy_ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
use bevy_ecs::world::{FilteredEntityMut, FilteredEntityRef};
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time};
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{Rollback, is_in_rollback};
use lightyear_frame_interpolation::FrameInterpolationSet;
use lightyear_replication::components::PrePredicted;
use lightyear_replication::prelude::{Confirmed, ReplicationReceiver};
use lightyear_replication::registry::ComponentKind;
use lightyear_replication::registry::registry::ComponentRegistry;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use tracing::{debug, debug_span, error, trace, trace_span, warn};

/// Responsible for re-running the FixedMain schedule a fixed number of times in order
/// to rollback the simulation to a previous state.
#[derive(Debug, Hash, PartialEq, Eq, Clone, ScheduleLabel)]
pub struct RollbackSchedule;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RollbackSet {
    // PreUpdate
    /// Check if rollback is needed
    Check,
    /// If any Predicted entity was marked as despawned, instead of despawning them we simply disabled the entity.
    /// If we do a rollback we want to restore those entities.
    RemoveDisable,
    /// Prepare rollback by snapping the current state to the confirmed state and clearing histories
    /// For pre-spawned entities, we just roll them back to their historical state.
    /// If they didn't exist in the rollback tick, despawn them
    Prepare,
    /// Perform rollback
    Rollback,
    /// Logic that returns right after the rollback is done:
    /// - Setting the VisualCorrection
    /// - Removing the Rollback component
    EndRollback,

    // PostUpdate
    /// After a rollback, instead of instantly snapping the visual state to the corrected state,
    /// we lerp the visual state from the previously predicted state to the corrected state
    VisualCorrection,
}

pub struct RollbackPlugin;

impl Plugin for RollbackPlugin {
    fn build(&self, app: &mut App) {
        // REFLECT
        app.register_type::<RollbackState>();

        // SETS
        app.configure_sets(
            PreUpdate,
            (
                RollbackSet::Check,
                RollbackSet::RemoveDisable.run_if(is_in_rollback),
                RollbackSet::Prepare.run_if(is_in_rollback),
                RollbackSet::Rollback.run_if(is_in_rollback),
                RollbackSet::EndRollback.run_if(is_in_rollback),
            )
                .chain()
                .in_set(PredictionSet::Rollback),
        );
        app.configure_sets(
            PostUpdate,
            // we add the correction error AFTER the interpolation was done
            // (which means it's also after we buffer the component for replication)
            RollbackSet::VisualCorrection
                .after(FrameInterpolationSet::Interpolate)
                .in_set(PredictionSet::All),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            (
                reset_input_rollback_tracker.after(RollbackSet::Check),
                remove_prediction_disable.in_set(RollbackSet::RemoveDisable),
                run_rollback.in_set(RollbackSet::Rollback),
                end_rollback.in_set(RollbackSet::EndRollback),
                #[cfg(feature = "metrics")]
                no_rollback
                    .after(RollbackSet::Check)
                    .in_set(PredictionSet::All)
                    .run_if(not(is_in_rollback)),
            ),
        );
    }

    /// Wait until every component has been registered in the ComponentRegistry
    fn finish(&self, app: &mut App) {
        // temporarily remove component_registry from the app to enable split borrows
        let component_registry = app
            .world_mut()
            .remove_resource::<ComponentRegistry>()
            .unwrap();
        let prediction_registry = app
            .world_mut()
            .remove_resource::<PredictionRegistry>()
            .unwrap();

        let check_rollback = (
            QueryParamBuilder::new(|builder| {
                builder.data::<&Confirmed>();
                builder.optional(|b| {
                    // include access to &C for all PredictionMode=Full components, if the components are replicated
                    prediction_registry
                        .prediction_map
                        .iter()
                        .filter(|(_, m)| m.sync_mode == PredictionMode::Full)
                        .filter_map(|(kind, _)| component_registry.component_metadata_map.get(kind))
                        .for_each(|m| {
                            b.ref_id(m.component_id);
                        });
                });
            }),
            QueryParamBuilder::new(|builder| {
                builder.data::<&Predicted>();
                builder.without::<Confirmed>();
                builder.without::<DeterministicPredicted>();
                builder.without::<DisableRollback>();
                builder.optional(|b| {
                    // include PredictionDisable entities (entities that are predicted and 'despawned'
                    // but we keep them around for rollback check)
                    b.data::<&PredictionDisable>();
                    // include access to &mut PredictionHistory<C> for all PredictionMode=Full components
                    prediction_registry
                        .prediction_map
                        .values()
                        .filter(|m| m.sync_mode == PredictionMode::Full)
                        .for_each(|m| {
                            b.mut_id(m.history_id.unwrap());
                        });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(check_rollback)
            .with_name("RollbackPlugin::check_rollback");

        app.add_systems(PreUpdate, check_rollback.in_set(RollbackSet::Check));

        app.insert_resource(component_registry);
        app.insert_resource(prediction_registry);
    }
}

#[derive(Component)]
/// Marker component used to indicate this entity is predicted (It has a PredictionHistory),
/// but it won't check for rollback from state updates.
///
/// This can be used to mark predicted non-networked entities in deterministic replication, or to stop a
/// state-replicated entity from being able to trigger rollbacks from state mismatch.
///
/// This entity will still get rolled back to its predicted history when a rollback happens.
pub struct DeterministicPredicted;

/// Marker component to indicate that the entity will be completely excluded from rollbacks.
/// It won't be part of rollback checks, and it won't be rolled back to a past state if a rollback happens.
#[derive(Component)]
pub struct DisableRollback;

#[derive(Component)]
/// Marker `Disabled` component inserted on `DisableRollback` entities during rollbacks so
/// that they are ignored from all queries
pub struct DisabledDuringRollback;

/// Check if we need to do a rollback.
/// We do this separately from `prepare_rollback` because even we stop the `check_rollback` function
/// early as soon as we find a mismatch, but we need to rollback all components to the original state.
fn check_rollback(
    // we want Query<&C, &Confirmed>
    confirmed_entities: Query<FilteredEntityRef>,
    // we want Query<&mut PredictionHistory<C>, With<Predicted>>
    // make sure to include disabled entities
    mut predicted_entities: Query<FilteredEntityMut>,
    receiver_query: Single<
        (
            Entity,
            &ReplicationReceiver,
            Option<&LastConfirmedInput>,
            &mut PredictionManager,
            &LocalTimeline,
        ),
        With<IsSynced<InputTimeline>>,
    >,
    prediction_registry: Res<PredictionRegistry>,
    system_ticks: SystemChangeTick,
    parallel_commands: ParallelCommands,
    mut commands: Commands,
) {
    // TODO: iterate through each archetype in parallel? using rayon

    // TODO: maybe have a sparse-set component with ConfirmedUpdated to quickly query only through predicted entities
    //  that received a confirmed update? Would the iteration even be faster? since entities with or without sparse-set
    //  would still be in the same table
    let (
        manager_entity,
        replication_receiver,
        last_confirmed_input,
        mut prediction_manager,
        local_timeline,
    ) = receiver_query.into_inner();
    let tick = local_timeline.tick();
    debug!(?tick, "Check rollback");
    let received_state = replication_receiver.has_received_this_frame();
    let mut skip_state_check = false;

    let do_rollback = move |rollback_tick: Tick,
                            prediction_manager: &PredictionManager,
                            commands: &mut Commands,
                            rollback: Rollback| {
        let delta = tick - rollback_tick;
        let max_rollback_ticks = prediction_manager.rollback_policy.max_rollback_ticks;
        if delta < 0 || delta as u16 > max_rollback_ticks {
            warn!(
                ?rollback_tick,
                ?tick,
                "Trying to do a rollback of {delta:?} ticks. The max is {max_rollback_ticks:?}! Aborting"
            );
            return;
        }
        // if prediction_manager.last_rollback_tick.is_some_and(|t| t >= rollback_tick)  {
        //     debug!(?rollback_tick, "Skipping rollback because we already did a roll back to a more recent tick");
        //     return
        // }
        prediction_manager.set_rollback_tick(rollback_tick);
        commands.entity(manager_entity).insert(rollback);
    };

    // if there we check for rollback on both state and input, state takes precedence
    match prediction_manager.rollback_policy.state {
        // if we received a state update, we don't check for mismatched and just set the rollback tick
        RollbackMode::Always => {
            if received_state && let Some(confirmed_ref) = confirmed_entities.iter().next() {
                debug!(
                    "Rollback because we have received a new confirmed state. (no mismatch check)"
                );
                let confirmed_tick = confirmed_ref.get::<Confirmed>().unwrap().tick;
                do_rollback(
                    confirmed_tick,
                    &prediction_manager,
                    &mut commands,
                    Rollback::FromState,
                );
                return;
            };
            skip_state_check = true;
        }
        RollbackMode::Check => {
            // no need to check for rollback if we didn't receive any state this frame
            if !received_state {
                skip_state_check = true;
            }
        }
        // set rollback from the LastConfirmedInput
        RollbackMode::Disabled => {
            skip_state_check = true;
        }
    }

    if !skip_state_check {
        predicted_entities.par_iter_mut().for_each(|mut predicted_mut| {
            let Some(confirmed) = predicted_mut.get::<Predicted>().and_then(|p| p.confirmed_entity) else {
                // skip if the confirmed entity does not exist
                return
            };
            let Ok(confirmed_ref) = confirmed_entities.get(confirmed) else {
                // skip if the confirmed entity does not exist
                return
            };
            // TODO: should we introduce a Rollback marker component?
            // we already know we are in rollback, no need to check again
            if prediction_manager.is_rollback() {
                return
            }

            // NOTE: do NOT use ref because the change ticks are incorrect within a system! Fixed in 0.17
            // let confirmed_component = get_ref::<Confirmed>(
            //     world,
            //     confirmed,
            //     system_ticks.last_run(),
            //     system_ticks.this_run(),
            // );

            // TODO: should we send an event when an entity receives an update? so that we check rollback
            //  only for entities that receive an update?
            // skip the entity if the replication group did not receive any updates
            let confirmed_ticks = confirmed_ref.get_change_ticks::<Confirmed>().unwrap();
            // we always want to rollback-check when Confirmed is added, to bring the entity to the correct timeline!
            if !confirmed_ticks.is_changed(system_ticks.last_run(), system_ticks.this_run()) {
                return
            };
            let confirmed_tick = confirmed_ref.get::<Confirmed>().unwrap().tick;

            if confirmed_tick > tick {
                debug!(
                    "Confirmed entity {:?} is at a tick in the future: {:?} compared to client timeline. Current tick: {:?}",
                    confirmed,
                    confirmed_tick,
                    tick
                );
                return;
            }

            // TODO: maybe pre-cache the components of the archetypes that we want to iterate over?
            //  it's not straightforward because we also want to handle rollbacks for components
            //  that were removed from the entity, which would not appear in the archetype
            for prediction_metadata in prediction_registry.prediction_map
                .values()
                .filter(|m| m.sync_mode == PredictionMode::Full)
                .take_while(|_| !prediction_manager.is_rollback()) {
                let check_rollback = prediction_metadata.full.as_ref().unwrap().check_rollback;
                if check_rollback(&prediction_registry, confirmed_tick, &confirmed_ref, &mut predicted_mut) {
                    debug!("Rollback because we have received a new confirmed state. (mismatch check)");
                    // During `prepare_rollback` we will reset the component to their values on `confirmed_tick`.
                    // Then when we do Rollback in PreUpdate, we will start by incrementing the tick, which will be equal to `confirmed_tick + 1`
                    parallel_commands.command_scope(|mut c| {
                        do_rollback(confirmed_tick, &prediction_manager, &mut c, Rollback::FromState);
                    });
                    return;
                }
            }
        });
    }

    // If we have found a state-based rollback, we are done.
    if prediction_manager.is_rollback() {
        debug!("Rollback was triggered by state, skipping input rollback checks");
    } else {
        // if we don't have state-based rollbacks, check for input-rollbacks
        match prediction_manager.rollback_policy.input {
            // If we have received any input message, rollback from the last confirmed input
            RollbackMode::Always => {
                if let Some(last_confirmed_input) = last_confirmed_input
                    && last_confirmed_input.received_input()
                {
                    debug!(
                        "Rollback because we have received a new remote input. (no mismatch check)"
                    );
                    // TODO: instead of rolling back to the last confirmed input, we could also just rollback
                    //  to the previous confirmed state (the inputs are just 'extra')
                    let rollback_tick = last_confirmed_input.tick.get();
                    do_rollback(
                        rollback_tick,
                        &prediction_manager,
                        &mut commands,
                        Rollback::FromInputs,
                    );
                }
            }
            // Rollback from any mismatched input
            RollbackMode::Check => {
                if prediction_manager.earliest_mismatch_input.has_mismatches() {
                    // we rollback to the tick right before the mismatch
                    let rollback_tick = prediction_manager.earliest_mismatch_input.tick.get() - 1;
                    debug!(
                        ?rollback_tick,
                        "Rollback because we have received a remote input that doesn't match our input buffer history"
                    );
                    do_rollback(
                        rollback_tick,
                        &prediction_manager,
                        &mut commands,
                        Rollback::FromInputs,
                    );
                }
            }
            _ => {}
        }
    }

    // if we have a rollback, despawn any PreSpawned entities that were spawned since the rollback tick
    // and were not matched with a remote entity
    // (they will get respawned during the rollback)
    if let Some(rollback_tick) = prediction_manager.get_rollback_start_tick() {
        debug!(
            ?rollback_tick,
            "Rollback! Despawning all PreSpawned entities spawned after that"
        );
        // 0. If the prespawned entity didn't exist at the rollback tick, despawn it
        // NOTE: if rollback happened at rollback_tick, then we will start running systems starting from rollback_tick + 1.
        //  so if the entity was spawned at tick >= rollback_tick + 1, we despawn it, and it can get respawned again
        for (_, hash) in prediction_manager
            .prespawn_tick_to_hash
            .drain_after(&(rollback_tick + 1))
        {
            if let Some(entities) = prediction_manager.prespawn_hash_to_entities.remove(&hash) {
                entities.into_iter().for_each(|entity| {
                    debug!(
                        ?entity,
                        "deleting pre-spawned entity because it was created after the rollback tick"
                    );
                    if let Ok(mut entity_commands) = commands.get_entity(entity) {
                        entity_commands.despawn();
                    }
                });
            }
        }
    }
}

// TODO: move this away from lightyear_prediction since LastConfirmedInput could be used without any prediction (lockstep)
/// Reset the trackers associated with RollbackMode::Input checks.
///
/// We do this here and not in `lightyear_input` because if we have multiple input types, the ticks
/// could be overwritten by each other.
///
/// This must run after the rollback check.
pub fn reset_input_rollback_tracker(
    client: Single<
        (
            &LocalTimeline,
            AnyOf<(&LastConfirmedInput, &PredictionManager)>,
        ),
        With<IsSynced<InputTimeline>>,
    >,
) {
    let (local_timeline, (last_confirmed_input, prediction_manager)) = client.into_inner();
    let tick = local_timeline.tick();

    // set a high value to the AtomicTick so we can then compute the minimum last_confirmed_tick among all clients
    if let Some(last_confirmed_input) = last_confirmed_input {
        last_confirmed_input.tick.0.store(
            (tick + 1000).0,
            bevy_platform::sync::atomic::Ordering::Relaxed,
        );
        last_confirmed_input
            .received_any_messages
            .store(false, bevy_platform::sync::atomic::Ordering::Relaxed);
    }
    if let Some(prediction_manager) = prediction_manager {
        // set a high value to the AtomicTick so we can then compute the minimum earliest_mismatch_tick among all clients
        prediction_manager.earliest_mismatch_input.tick.0.store(
            (tick + 1000).0,
            bevy_platform::sync::atomic::Ordering::Relaxed,
        );
        prediction_manager
            .earliest_mismatch_input
            .has_mismatches
            .store(false, bevy_platform::sync::atomic::Ordering::Relaxed);
    }
}

// TODO: maybe restore only the ones for which the Confirmed entity is not disabled?
/// Before we start preparing for rollback, restore any PredictionDisable predicted entity
pub(crate) fn remove_prediction_disable(
    mut commands: Commands,
    query: Query<Entity, (With<Predicted>, With<PredictionDisable>)>,
) {
    query.iter().for_each(|e| {
        commands.entity(e).try_remove::<PredictionDisable>();
    });
}

// pub(crate) fn prepare_rollback_full(
//     prediction_registry: Res<PredictionRegistry>,
//     component_registry: Res<ComponentRegistry>,
//     mut commands: Commands,
//     // We also snap the value of the component to the server state if we are in rollback
//     // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
//     mut predicted_query: Query<FilteredEntityMut>,
//     confirmed_query: Query<FilteredEntityRef>,
//     // mut predicted_query: Query<
//     //     (
//     //         Option<&mut C>,
//     //         &mut PredictionHistory<C>,
//     //         Has<DisableRollback>,
//     //     ),
//     //     (With<Predicted>, Without<Confirmed>, Without<PreSpawned>),
//     // >,
//     // TODO: have a way to only get the updates of entities that are predicted? i.e. add ConfirmedPredicted
//     // confirmed_query: Query<(Entity, Option<&C>, Ref<Confirmed>)>,
//     manager_query: Single<(&LocalTimeline, &PredictionManager, &Rollback)>,
// ) {
//     let (timeline, manager, rollback) = manager_query.into_inner();
//     let current_tick = timeline.tick();
//     let _span = trace_span!("prepare_rollback", tick = ?current_tick);
//     let rollback_tick = manager.get_rollback_start_tick().unwrap();
//
//     // TODO: cache the list of predicted full components on that entity.
//     predicted_query.par_iter_mut().for_each(|mut predicted_mut| {
//         let history_id: ComponentId;
//         // let history_id = prediction_registry.prediction_map.get(predicted_mut.kind())
//         let mut predicted_history = predicted_mut.get_mut_by_id(history_id).unwrap();
//
//         // 1. Get the history value and clear the history
//         let original_predicted_value = predicted_history.pop_until_tick(rollback_tick);
//         predicted_history.clear();
//
//
//
//     });
//
// }

/// If there is a mismatch, prepare rollback for all components
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback<C: SyncComponent>(
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
    mut commands: Commands,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (
            Entity,
            Option<&mut C>,
            &mut PredictionHistory<C>,
            AnyOf<(
                &Predicted,
                &PreSpawned,
                &DeterministicPredicted,
                &PrePredicted,
            )>,
        ),
        (Without<Confirmed>, Without<DisableRollback>),
    >,
    // TODO: have a way to only get the updates of entities that are predicted? i.e. add ConfirmedPredicted
    confirmed_query: Query<(Option<&C>, Ref<Confirmed>)>,
    manager_query: Single<(&LocalTimeline, &PredictionManager, &Rollback)>,
) {
    let kind = core::any::type_name::<C>();
    let (timeline, manager, rollback) = manager_query.into_inner();
    let current_tick = timeline.tick();
    let _span = trace_span!("prepare_rollback", tick = ?current_tick, kind = ?kind).entered();
    let rollback_tick = manager.get_rollback_start_tick().unwrap();

    let is_non_networked = component_registry
        .component_metadata_map
        .get(&ComponentKind::of::<C>())
        .is_none_or(|m| m.serialization.is_none());
    for (
        predicted_entity,
        predicted_component,
        mut predicted_history,
        (predicted, prespawned, disable_state_rollback, pre_predicted),
    ) in predicted_query.iter_mut()
    {
        // 1. we need to clear the history so we can write a new one
        let original_predicted_value = predicted_history.clear_except_tick(rollback_tick);

        // 2. find the correct value to rollback to, and whether or not it's the Confirmed state or the PredictionHistory state
        // If DisableStateRollback, PrePredicted, Prespawned -> we just rollback to the PredictionHistory state, not the Confirmed state.
        let (correct_value, from_history) = match (
            rollback,
            disable_state_rollback.is_some()
                || pre_predicted.is_some()
                || prespawned.is_some()
                || is_non_networked,
        ) {
            // we will rollback to the confirmed state
            (Rollback::FromState, false) => {
                let Some(predicted) = predicted else {
                    error!("Entity needs a Predicted component to handle Rollback::FromState");
                    continue;
                };
                let Some(confirmed_entity) = predicted.confirmed_entity else {
                    error!("Predicted entity has no corresponding confirmed entity for rollback!");
                    continue;
                };
                let Ok((confirmed_component, confirmed)) = confirmed_query.get(confirmed_entity)
                else {
                    continue;
                };
                // For state-based rollback, we clear the history even for the rollback tick
                // since it will be replaced with the state-replicated value
                predicted_history.clear();
                trace!(?predicted_entity, "Rollback to the confirmed state");
                // Confirm that we are in rollback, with the correct tick
                debug_assert_eq!(
                    rollback_tick, confirmed.tick,
                    "The rollback tick (LEFT) does not match the confirmed tick (RIGHT) for confirmed entity {confirmed_entity:?}. Are all predicted entities in the same replication group?",
                );
                (confirmed_component, false)
            }
            // we will rollback to the value stored in the PredictionHistory
            _ => {
                trace!(
                    ?predicted_entity,
                    "Rollback to the value stored in the PredictionHistory"
                );
                (
                    original_predicted_value.as_ref().and_then(|v| match v {
                        HistoryState::Updated(v) => Some(v),
                        _ => None,
                    }),
                    true,
                )
            }
        };

        let mut entity_mut = commands.entity(predicted_entity);

        // 3. we update the state to the Corrected state
        match correct_value {
            // confirm does not exist, remove on predicted
            None => {
                predicted_history.add_remove(rollback_tick);
                entity_mut.try_remove::<C>();
                trace!(
                    ?from_history,
                    "Removing component from predicted entity for rollback"
                );
            }
            // confirm exist, update or insert on predicted
            Some(correct) => {
                let mut correct_value = correct.clone();
                // if the correct value is from the PredictionHistory, we already did entity mapping and it's
                // already part of the history!
                if !from_history {
                    let _ = manager.map_entities(&mut correct_value, component_registry.as_ref());
                    predicted_history.add_update(rollback_tick, correct_value.clone());
                    trace!("Add {rollback_tick:?} to history");
                }
                match predicted_component {
                    None => {
                        debug!("Re-adding deleted Full component to predicted");
                        entity_mut.insert(correct_value);
                    }
                    Some(mut predicted_component) => {
                        // keep track of the current visual value so we can smooth the correction
                        if prediction_registry.has_correction::<C>() {
                            entity_mut.insert(PreviousVisual(predicted_component.clone()));
                            trace!(
                                previous_visual = ?predicted_component,
                                "Storing PreviousVisual for correction"
                            );
                        }

                        // update the component to the corrected value
                        *predicted_component = correct_value;
                    }
                };
            }
        };
    }
}

// Revert `resource` to its value at the tick that the incoming rollback will rollback to.
pub(crate) fn prepare_rollback_resource<R: Resource + Clone>(
    mut commands: Commands,
    prediction_manager: Single<&PredictionManager, With<Rollback>>,
    resource: Option<ResMut<R>>,
    mut history: ResMut<ResourceHistory<R>>,
) {
    let kind = core::any::type_name::<R>();
    let _span = trace_span!("client prepare rollback for resource", ?kind);

    let Some(rollback_tick) = prediction_manager.get_rollback_start_tick() else {
        error!("prepare_rollback_resource should only be called when we are in rollback");
        return;
    };

    let history_value = history.clear_except_tick(rollback_tick);

    // 1. restore the resource to the historical value
    match history_value {
        None | Some(HistoryState::Removed) => {
            if resource.is_some() {
                debug!(
                    ?kind,
                    "Resource didn't exist at time of rollback, removing it"
                );
                // the resource didn't exist at the time, remove it!
                commands.remove_resource::<R>();
            }
        }
        Some(HistoryState::Updated(r)) => {
            // the resource existed at the time, restore it!
            if let Some(mut resource) = resource {
                // update the resource to the corrected value
                *resource = r.clone();
            } else {
                debug!(
                    ?kind,
                    "Resource for predicted entity existed at time of rollback, inserting it"
                );
                commands.insert_resource(r.clone());
            }
        }
    }
}

/// Return a fixed time that represents rollbacking `current_fixed_time` by
/// `num_rollback_ticks` ticks. The returned fixed time's overstep is zero.
///
/// This function assumes that `current_fixed_time`'s timestep remained the
/// same for the past `num_rollback_ticks` ticks.
fn rollback_fixed_time(current_fixed_time: &Time<Fixed>, num_rollback_ticks: i16) -> Time<Fixed> {
    let mut rollback_fixed_time = Time::<Fixed>::from_duration(current_fixed_time.timestep());
    if num_rollback_ticks <= 0 {
        debug!("Cannot rollback fixed time by {} ticks", num_rollback_ticks);
        return rollback_fixed_time;
    }
    // Fixed time's elapsed time's is set to the fixed time's delta before any
    // fixed system has run in an app, see
    // `bevy_time::fixed::run_fixed_main_schedule()`. If elapsed time is zero
    // that means no tick has run.
    if current_fixed_time.elapsed() < current_fixed_time.timestep() {
        error!("Current elapsed fixed time is less than the fixed timestep");
        return rollback_fixed_time;
    }

    // Difference between the current time and the time of the first tick of
    // the rollback.
    let rollback_time_offset = (num_rollback_ticks - 1) as u32 * rollback_fixed_time.timestep();

    let rollback_elapsed_time = current_fixed_time
        .elapsed()
        .saturating_sub(rollback_time_offset);
    rollback_fixed_time
        .advance_to(rollback_elapsed_time.saturating_sub(rollback_fixed_time.timestep()));
    // Time<Fixed>::delta is set to the value provided in `advance_by` (or
    // `advance_to`), so we want to call
    // `advance_by(rollback_fixed_time.timestep())` at the end to set the delta
    // value that is expected.
    rollback_fixed_time.advance_by(rollback_fixed_time.timestep());

    rollback_fixed_time
}

pub(crate) fn run_rollback(world: &mut World) {
    let (entity, mut local_timeline, prediction_manager) = world
        .query::<(Entity, &mut LocalTimeline, &PredictionManager)>()
        .single_mut(world)
        .unwrap();

    // NOTE: all predicted entities should be on the same tick!
    // TODO: might not need to check the state, because we only run this system if we are in rollback
    let current_tick = local_timeline.tick();
    let rollback_start_tick = prediction_manager
        .get_rollback_start_tick()
        .expect("we should be in rollback");

    // NOTE: we reverted all components to the end of `current_roll
    let num_rollback_ticks = current_tick - rollback_start_tick;
    // reset the local timeline to be at the end of rollback_start_tick and we want to reach the end of current_tick
    local_timeline.apply_delta((-num_rollback_ticks).into());
    debug!(
        "Rollback between {:?} and {:?}",
        rollback_start_tick, current_tick
    );
    #[cfg(feature = "metrics")]
    {
        metrics::counter!("prediction::rollbacks::count").increment(1);
        metrics::gauge!("prediction::rollbacks::event").set(1);
        metrics::gauge!("prediction::rollbacks::ticks").set(num_rollback_ticks);
    }

    // Keep track of the generic time resource so it can be restored after the rollback.
    let time_resource = *world.resource::<Time>();

    // Rollback the fixed time resource in preparation for the rollback.
    let current_fixed_time = *world.resource::<Time<Fixed>>();
    *world.resource_mut::<Time<Fixed>>() =
        rollback_fixed_time(&current_fixed_time, num_rollback_ticks);

    // TODO: should we handle Time<Physics> and Time<Subsets> in any way?
    //  we might need to rollback them if the physics time is paused
    //  otherwise setting Time<()> to Time<Fixed> should be enough
    //  as Time<Physics> uses Time<()>'s delta

    // Insert the DisabledDuringRollback component on all entities that have the DisableRollback component
    let disabled_entities = world
        .query_filtered::<Entity, With<DisableRollback>>()
        .iter(world)
        .collect::<Vec<_>>();
    disabled_entities.iter().for_each(|entity| {
        world.entity_mut(*entity).insert(DisabledDuringRollback);
    });

    // Run the fixed update schedule (which should contain ALL
    // predicted/rollback components and resources). This is similar to what
    // `bevy_time::fixed::run_fixed_main_schedule()` does
    for i in 0..num_rollback_ticks {
        // we add 1 here because running FixedUpdate will start by incrementing the tick
        let rollback_tick = rollback_start_tick + i + 1;
        let _span = debug_span!("rollback", tick = ?rollback_tick).entered();
        debug!(?rollback_tick, "rollback");
        // Set the rollback tick's generic time resource to the fixed time
        // resource that was just advanced.
        *world.resource_mut::<Time>() = world.resource::<Time<Fixed>>().as_generic();

        // TODO: if we are in rollback, there are some FixedUpdate systems that we don't want to re-run ??
        //  for example we only want to run the physics on non-confirmed entities
        world.run_schedule(FixedMain);

        // Manually advanced fixed time because `run_schedule(FixedMain)` does
        // not.
        let timestep = world.resource::<Time<Fixed>>().timestep();
        world.resource_mut::<Time<Fixed>>().advance_by(timestep);
    }

    // Remove the DisabledDuringRollback component on all entities that have it
    disabled_entities.into_iter().for_each(|entity| {
        world.entity_mut(entity).remove::<DisabledDuringRollback>();
    });

    // Restore the fixed time resource.
    // `current_fixed_time` and the fixed time resource in use (e.g. the
    // rollback fixed time) should be the same after the rollback except that
    // `current_fixed_time` may have an overstep. Use `current_fixed_time` so
    // its overstep isn't lost.
    *world.resource_mut::<Time<Fixed>>() = current_fixed_time;

    // Restore the generic time resource.
    *world.resource_mut::<Time>() = time_resource;
    debug!("Finished rollback. Current tick: {:?}", current_tick);

    let mut metrics = world.get_resource_mut::<PredictionMetrics>().unwrap();
    metrics.rollbacks += 1;
    metrics.rollback_ticks += num_rollback_ticks as u32;
}

pub(crate) fn end_rollback(
    prediction_manager: Single<(Entity, &PredictionManager), With<Rollback>>,
    mut commands: Commands,
) {
    let (entity, prediction_manager) = prediction_manager.into_inner();
    prediction_manager.set_non_rollback();
    commands.entity(entity).remove::<Rollback>();
}

#[cfg(feature = "metrics")]
pub(crate) fn no_rollback() {
    metrics::gauge!("prediction::rollbacks::event").set(0);
    metrics::gauge!("prediction::rollbacks::ticks").set(0);
}

/// Track whether we are in rollback or not
#[derive(Debug, Default, Reflect)]
pub enum RollbackState {
    /// We are not in a rollback state
    #[default]
    Default,
    /// We should do a rollback starting from this tick
    ///
    /// i.e. the predicted component values will be reverted to this tick, and we will start running FixedUpdate from the next tick
    RollbackStart(Tick),
}
