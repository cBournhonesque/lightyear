use super::predicted_history::PredictionHistory;
use super::resource_history::ResourceHistory;
use super::{Predicted, PredictionMode, SyncComponent};
use crate::correction::Correction;
use crate::despawn::PredictionDisable;
use crate::diagnostics::PredictionMetrics;
use crate::manager::{PredictionManager, RollbackMode};
use crate::plugin::PredictionSet;
use crate::prespawn::PreSpawned;
use crate::registry::PredictionRegistry;
use bevy_app::{App, FixedMain, Plugin, PreUpdate};
use bevy_ecs::component::Mutable;
use bevy_ecs::entity::EntityHashSet;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
use bevy_ecs::world::{FilteredEntityMut, FilteredEntityRef};
use bevy_time::{Fixed, Time};
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::timeline::Rollback;
use lightyear_replication::prelude::{Confirmed, ReplicationReceiver};
use lightyear_replication::registry::registry::ComponentRegistry;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use tracing::{debug, debug_span, error, trace, trace_span};

pub struct RollbackPlugin;

impl Plugin for RollbackPlugin {
    fn build(&self, app: &mut App) {}

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
                    // include access to &C for all PredictionMode=Full components
                    prediction_registry
                        .prediction_map
                        .iter()
                        .filter(|(_, m)| m.sync_mode == PredictionMode::Full)
                        .map(|(kind, _)| component_registry.kind_to_component_id[kind])
                        .for_each(|id| {
                            b.ref_id(id);
                        });
                });
            }),
            QueryParamBuilder::new(|builder| {
                builder.data::<&Predicted>();
                builder.without::<Confirmed>();
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
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(check_rollback)
            .with_name("RollbackPlugin::check_rollback");

        app.add_systems(
            PreUpdate,
            check_rollback.in_set(PredictionSet::CheckRollback),
        );

        app.insert_resource(component_registry);
        app.insert_resource(prediction_registry);
    }
}

#[derive(Component)]
/// Marker component used to indicate that an entity:
/// - won't trigger rollbacks
/// - will never have mispredictions. During rollbacks we will revert the entity to the
///   past value from the PredictionHistory instead of the confirmed value
pub struct DisableRollback;

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
            &PredictionManager,
            &LocalTimeline,
        ),
        With<IsSynced<InputTimeline>>,
    >,
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
    system_ticks: SystemChangeTick,
    parallel_commands: ParallelCommands,
    mut commands: Commands,
) {
    // TODO: iterate through each archetype in parallel? using rayon

    // TODO: maybe have a sparse-set component with ConfirmedUpdated to quickly query only through predicted entities
    //  that received a confirmed update? Would the iteration even be faster? since entities with or without sparse-set
    //  would still be in the same table
    let (manager_entity, replication_receiver, prediction_manager, local_timeline) =
        receiver_query.into_inner();
    let tick = local_timeline.tick();
    let received_state = replication_receiver.has_received_this_frame();
    let mut skip_state_check = false;

    // if there we check for rollback on both state and input, state takes precedence
    match prediction_manager.rollback_policy.state {
        // if we received a state update, we don't check for mismatched and just set the rollback tick
        RollbackMode::Always => {
            if received_state && let Some(confirmed_ref) = confirmed_entities.iter().next() {
                trace!(
                    "Rollback because we have received a new confirmed state. (no mismatch check)"
                );
                let confirmed_tick = confirmed_ref.get::<Confirmed>().unwrap().tick;
                prediction_manager.set_rollback_tick(confirmed_tick);
                commands.entity(manager_entity).insert(Rollback);
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
            for (id, prediction_metadata) in prediction_registry.prediction_map
                .iter()
                .filter(|(_, m)| m.sync_mode == PredictionMode::Full)
                .map(|(kind, m)| (component_registry.kind_to_component_id[kind], m))
                .take_while(|_| !prediction_manager.is_rollback()) {
                if (prediction_metadata.check_rollback)(&prediction_registry, confirmed_tick, &confirmed_ref, &mut predicted_mut) {
                    trace!("Rollback because we have received a new confirmed state. (mismatch check)");
                    // During `prepare_rollback` we will reset the component to their values on `confirmed_tick`.
                    // Then when we do Rollback in PreUpdate, we will start by incrementing the tick, which will be equal to `confirmed_tick + 1`
                    prediction_manager.set_rollback_tick(confirmed_tick);
                    parallel_commands.command_scope(|mut c| {
                        c.entity(manager_entity).insert(Rollback);
                    });
                    return;
                }
            }
        });
    }

    // If we have found a state-based rollback, we are done.
    if prediction_manager.is_rollback() {
        return;
    }

    // if we don't have state-based rollbacks, check for input-rollbacks
    match prediction_manager.rollback_policy.input {
        // If we have received any input message, rollback from the last confirmed input
        RollbackMode::Always => {
            if prediction_manager.last_confirmed_input.received_input() {
                trace!("Rollback because we have received a new remote input. (no mismatch check)");
                // TODO: instead of rolling back to the last confirmed input, we could also just rollback
                //  to the previous confirmed state (the inputs are just 'extra')
                let rollback_tick = prediction_manager.last_confirmed_input.tick.get();
                prediction_manager.set_rollback_tick(rollback_tick);
                commands.entity(manager_entity).insert(Rollback);
            }
        }
        // Rollback from any mismatched input
        RollbackMode::Check => {
            if let Some(rollback_tick) = prediction_manager.get_input_rollback_start_tick() {
                trace!("Rollback because we have received a new remote input. (mismatch check)");
                prediction_manager.set_rollback_tick(rollback_tick);
                commands.entity(manager_entity).insert(Rollback);
            }
        }
        _ => {}
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

/// If there is a mismatch, prepare rollback for all components
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback<C: SyncComponent>(
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (
            Option<&mut C>,
            &mut PredictionHistory<C>,
            Option<&mut Correction<C>>,
            Has<DisableRollback>,
        ),
        (With<Predicted>, Without<Confirmed>, Without<PreSpawned>),
    >,
    confirmed_query: Query<(Entity, Option<&C>, Ref<Confirmed>)>,
    manager_query: Single<(&LocalTimeline, &PredictionManager), With<Rollback>>,
) {
    let kind = core::any::type_name::<C>();
    let (timeline, manager) = manager_query.into_inner();
    let current_tick = timeline.tick();
    let _span = trace_span!("prepare_rollback", tick = ?current_tick, kind = ?kind);
    for (confirmed_entity, confirmed_component, confirmed) in confirmed_query.iter() {
        let rollback_tick = confirmed.tick;

        // ignore the confirmed entities that only have interpolation
        // TODO: separate ConfirmedPredicted and ConfirmedInterpolated!
        let Some(predicted_entity) = confirmed.predicted else {
            continue;
        };

        // 0. Confirm that we are in rollback, with the correct tick
        debug_assert_eq!(
            manager.get_rollback_start_tick(),
            Some(rollback_tick),
            "The rollback tick (LEFT) does not match the confirmed tick (RIGHT) for confirmed entity {confirmed_entity:?}. Are all predicted entities in the same replication group?",
        );

        // 1. Get the predicted entity, and its history
        let Ok((predicted_component, mut predicted_history, mut correction, disable_rollback)) =
            predicted_query.get_mut(predicted_entity)
        else {
            debug!(
                "Predicted entity {:?} was not found when preparing rollback for {:?}",
                confirmed.predicted,
                core::any::type_name::<C>()
            );
            continue;
        };

        // 2. we need to clear the history so we can write a new one
        let original_predicted_value = predicted_history.pop_until_tick(rollback_tick);
        predicted_history.clear();

        // if rollback is disabled, we will restore the component to its past value from the prediction history
        let correct_value = if disable_rollback {
            trace!(
                ?predicted_entity,
                "DisableRollback is present! Get confirmed value from PredictionHistory"
            );
            original_predicted_value.as_ref().and_then(|v| match v {
                HistoryState::Updated(v) => Some(v),
                _ => None,
            })
        } else {
            confirmed_component
        };

        // SAFETY: we know the predicted entity exists
        let mut entity_mut = commands.entity(predicted_entity);

        // 3. we update the state to the Corrected state
        // NOTE: visually, we will use the CorrectionFn to interpolate between the current Predicted state and the Corrected state
        //  even though for other purposes (physics, etc.) we switch directly to the Corrected state
        match correct_value {
            // confirm does not exist, remove on predicted
            None => {
                predicted_history.add_remove(rollback_tick);
                entity_mut.try_remove::<C>();
            }
            // confirm exist, update or insert on predicted
            Some(confirmed_component) => {
                let mut rollbacked_predicted_component = confirmed_component.clone();
                // when rollback is disabled, the correct value is taken from the prediction history
                // so no need to map entities
                if !disable_rollback {
                    let _ = manager.map_entities(
                        &mut rollbacked_predicted_component,
                        component_registry.as_ref(),
                    );
                }
                // TODO: do i need to add this to the history?
                predicted_history.add_update(rollback_tick, rollbacked_predicted_component.clone());
                match predicted_component {
                    None => {
                        debug!("Re-adding deleted Full component to predicted");
                        entity_mut.insert(rollbacked_predicted_component);
                    }
                    Some(mut predicted_component) => {
                        // no need to do a correction if the predicted value from the history
                        // is the same as the newly received confirmed value
                        // (this can happen if you predict 2 entities A and B.
                        //  A needs a rollback, but B was predicted correctly. In that case you don't want
                        //  to do a correction for B)
                        if let Some(HistoryState::Updated(prev)) = original_predicted_value {
                            // TODO: use should_rollback function?
                            if rollbacked_predicted_component == prev {
                                // instead we just rollback the component value without correction
                                *predicted_component = rollbacked_predicted_component.clone();
                                continue;
                            }
                        }

                        // insert the Correction information only if the component exists on both confirmed and predicted
                        let correction_ticks = ((current_tick - rollback_tick) as f32
                            * manager.correction_ticks_factor)
                            .round() as i16;
                        // no need to add the Correction if the correction is instant
                        if correction_ticks != 0 && prediction_registry.has_correction::<C>() {
                            let final_correction_tick = current_tick + correction_ticks;
                            if let Some(correction) = correction.as_mut() {
                                trace!("updating existing correction");
                                // if there is a correction, start the correction again from the previous
                                // visual state to avoid glitches
                                correction.original_prediction =
                                    core::mem::take(&mut correction.current_visual)
                                        .unwrap_or_else(|| predicted_component.clone());
                                correction.original_tick = current_tick;
                                correction.final_correction_tick = final_correction_tick;
                                correction.current_correction = None;
                            } else {
                                trace!(
                                    kind = ?core::any::type_name::<C>(),
                                    ?current_tick,
                                    ?final_correction_tick,
                                    "inserting new correction"
                                );
                                entity_mut.insert(Correction {
                                    original_prediction: predicted_component.clone(),
                                    original_tick: current_tick,
                                    final_correction_tick,
                                    current_visual: None,
                                    current_correction: None,
                                });
                            }
                        }

                        // update the component to the corrected value
                        *predicted_component = rollbacked_predicted_component;
                    }
                };
            }
        };
    }
}

// TODO: handle disable rollback, by combining all prepare_rollback systems into one
/// For prespawned predicted entities, we do not have a Confirmed component,
/// we just rollback the entity to the previous state
/// - entities that did not exist at the rollback tick are despawned (and should be respawned during rollback)
/// - component that were inserted since rollback are removed
/// - components that were removed since rollback are inserted
/// - entities that were spawned since rollback are despawned
/// - no need to do correction because we don't have a Confirmed state to correct towards
/// - TODO: entities that were despawned since rollback are respawned (maybe just via using prediction_despawn()?)
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback_prespawn<C: SyncComponent>(
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (Entity, Option<&mut C>, &mut PredictionHistory<C>),
        (With<PreSpawned>, Without<Confirmed>, Without<Predicted>),
    >,
    // TODO: have a way to make these systems run in parallel
    //  - either by using RwLock in PredictionManager
    //  - or by using a system that iterates through archetypes, a la replicon?
    mut prediction_manager: Single<&mut PredictionManager, With<Rollback>>,
) {
    let kind = core::any::type_name::<C>();
    let _span = trace_span!("client prepare rollback for pre-spawned entities");

    let Some(rollback_tick) = prediction_manager.get_rollback_start_tick() else {
        error!("prepare_rollback_prespawn should only be called when we are in rollback");
        return;
    };

    // 0. If the prespawned entity didn't exist at the rollback tick, despawn it
    // TODO: also handle deleting pre-predicted entities!
    // NOTE: if rollback happened at current_tick - 1, then we will start running systems starting from current_tick.
    //  so if the entity was spawned at tick >= current_tick, we despawn it, and it can get respawned again
    let mut entities_to_despawn = EntityHashSet::default();
    for (_, hash) in prediction_manager
        .prespawn_tick_to_hash
        .drain_after(&(rollback_tick + 1))
    {
        if let Some(entities) = prediction_manager.prespawn_hash_to_entities.remove(&hash) {
            entities_to_despawn.extend(entities);
        }
    }
    entities_to_despawn.iter().for_each(|entity| {
        debug!(
            ?entity,
            "deleting pre-spawned entity because it was created after the rollback tick"
        );
        if let Ok(mut entity_commands) = commands.get_entity(*entity) {
            entity_commands.despawn();
        }
    });

    for (prespawned_entity, predicted_component, mut predicted_history) in
        predicted_query.iter_mut()
    {
        if entities_to_despawn.contains(&prespawned_entity) {
            continue;
        }

        // 1. restore the component to the historical value
        match predicted_history.pop_until_tick(rollback_tick) {
            None | Some(HistoryState::Removed) => {
                if predicted_component.is_some() {
                    debug!(
                        ?prespawned_entity,
                        ?kind,
                        "Component for prespawned entity didn't exist at time of rollback, removing it"
                    );
                    // the component didn't exist at the time, remove it!
                    commands.entity(prespawned_entity).try_remove::<C>();
                }
            }
            Some(HistoryState::Updated(c)) => {
                // the component existed at the time, restore it!
                if let Some(mut predicted_component) = predicted_component {
                    // update the component to the corrected value
                    *predicted_component = c.clone();
                } else {
                    debug!(
                        ?prespawned_entity,
                        ?kind,
                        "Component for prespawned entity existed at time of rollback, inserting it"
                    );
                    commands.entity(prespawned_entity).insert(c);
                }
            }
        }

        // 2. we need to clear the history so we can write a new one
        predicted_history.clear();
    }
}

/// For non-networked components, it's exactly the same as for pre-spawned entities:
/// we do not have a Confirmed component, so we don't revert back to the Confirmed value,
/// we revert to the value read from the `PredictedHistory` instead
/// - entities that did not exist at the rollback tick are despawned (and should be respawned during rollback)
/// - component that were inserted since rollback are removed
/// - components that were removed since rollback are inserted
/// - entities that were spawned since rollback are despawned
/// - no need to do correction because we don't have a Confirmed state to correct towards
/// - TODO: entities that were despawned since rollback are respawned (maybe just via using prediction_despawn()?)
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback_non_networked<
    C: Component<Mutability = Mutable> + PartialEq + Clone,
>(
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (Entity, Option<&mut C>, &mut PredictionHistory<C>),
        With<Predicted>,
    >,
    prediction_manager: Single<&PredictionManager, With<Rollback>>,
) {
    let kind = core::any::type_name::<C>();
    let _span = trace_span!("client prepare rollback for non networked component", ?kind);

    let Some(rollback_tick) = prediction_manager.get_rollback_start_tick() else {
        error!(
            "prepare_rollback_non_networked_components should only be called when we are in rollback"
        );
        return;
    };

    // 0. If the entity didn't exist at the rollback tick, despawn it
    // TODO? or is it handled for us?
    for (entity, component, mut history) in predicted_query.iter_mut() {
        // 1. restore the component to the historical value
        match history.pop_until_tick(rollback_tick) {
            None | Some(HistoryState::Removed) => {
                if component.is_some() {
                    debug!(
                        ?entity,
                        ?kind,
                        "Non-networked component for predicted entity didn't exist at time of rollback, removing it"
                    );
                    // the component didn't exist at the time, remove it!
                    commands.entity(entity).try_remove::<C>();
                }
            }
            Some(HistoryState::Updated(c)) => {
                // the component existed at the time, restore it!
                if let Some(mut component) = component {
                    // update the component to the corrected value
                    *component = c.clone();
                } else {
                    debug!(
                        ?entity,
                        ?kind,
                        "Non-networked component for predicted entity existed at time of rollback, inserting it"
                    );
                    commands.entity(entity).insert(c);
                }
            }
        }

        // 2. we need to clear the history so we can write a new one
        history.clear();
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

    // 1. restore the resource to the historical value
    match history.pop_until_tick(rollback_tick) {
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
                commands.insert_resource(r);
            }
        }
    }

    // 2. we need to clear the history so we can write a new one
    history.clear();
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

    // revert the state of Rollback for the next frame
    let prediction_manager = world
        .query::<&mut PredictionManager>()
        .single_mut(world)
        .unwrap();
    prediction_manager.set_non_rollback();
    world.entity_mut(entity).remove::<Rollback>();
}

#[cfg(feature = "metrics")]
pub(crate) fn no_rollback() {
    metrics::gauge!("prediction::rollbacks::event").set(0);
    metrics::gauge!("prediction::rollbacks::ticks").set(0);
}
