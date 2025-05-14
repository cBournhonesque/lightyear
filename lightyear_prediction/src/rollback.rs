use super::predicted_history::PredictionHistory;
use super::resource_history::ResourceHistory;
use super::{Predicted, PredictionMode, SyncComponent};
use crate::correction::Correction;
use crate::despawn::PredictionDisable;
use crate::diagnostics::PredictionMetrics;
use crate::manager::PredictionManager;
use crate::plugin::PredictionSet;
use crate::prespawn::PreSpawned;
use crate::registry::PredictionRegistry;
use bevy::app::FixedMain;
use bevy::ecs::component::Mutable;
use bevy::ecs::entity::hash_set::EntityHashSet;
use bevy::ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
use bevy::ecs::world::{FilteredEntityMut, FilteredEntityRef};
use bevy::prelude::*;
use bevy::time::{Fixed, Time};
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::timeline::Rollback;
use lightyear_replication::prelude::{Confirmed, ReplicationReceiver};
use lightyear_replication::registry::registry::ComponentRegistry;

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
        )
            .build_state(app.world_mut())
            .build_system(check_rollback);

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
    receiver_query: Single<(
        Entity,
        &ReplicationReceiver,
        &PredictionManager,
        &LocalTimeline,
    )>,
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
    system_ticks: SystemChangeTick,
    commands: ParallelCommands,
) {
    // TODO: iterate through each archetype in parallel? using rayon

    // TODO: maybe have a sparse-set component with ConfirmedUpdated to quickly query only through predicted entities
    //  that received a confirmed update? Would the iteration even be faster? since entities with or without sparse-set
    //  would still be in the same table
    let (manager_entity, replication_receiver, prediction_manager, local_timeline) =
        receiver_query.into_inner();
    let tick = local_timeline.tick();
    // no need to check for rollback if we didn't receive any packet
    if !replication_receiver.has_received_this_frame() {
        return;
    }
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
                // During `prepare_rollback` we will reset the component to their values on `confirmed_tick`.
                // Then when we do Rollback in PreUpdate, we will start by incrementing the tick, which will be equal to `confirmed_tick + 1`
                prediction_manager.set_rollback_tick(confirmed_tick);
                commands.command_scope(|mut c| {
                    c.entity(manager_entity).insert(Rollback);
                });
                return;
            }
        }
    })
}

// TODO: maybe restore only the ones for which the Confirmed entity is not disabled?
/// Before we start preparing for rollback, restore any PredictionDisable predicted entity
pub(crate) fn remove_prediction_disable(
    mut commands: Commands,
    query: Query<Entity, (With<Predicted>, With<PredictionDisable>)>,
) {
    query.iter().for_each(|e| {
        commands.entity(e).remove::<PredictionDisable>();
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

        // // 0. Confirm that we are in rollback.
        // // NOTE: currently all predicted entities must be in the same replication group because I do not know how
        // //  to do a 'partial' rollback for only some entities
        // let Some(RollbackState::ShouldRollback { current_tick }) = rollback.state else {
        //     continue;
        // };
        // // careful, we added 1 to the tick in the check_rollback stage...
        // let tick = Tick(*current_tick - 1);

        let Some(predicted_entity) = confirmed.predicted else {
            continue;
        };

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
                entity_mut.remove::<C>();
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
                    commands.entity(prespawned_entity).remove::<C>();
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
                    commands.entity(entity).remove::<C>();
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
        info!(?rollback_tick, "rollback");
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

#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;
    use bevy::prelude::Entity;
    use core::time::Duration;

    /// Helper function to simulate that we received a server message
    pub(crate) fn received_confirmed_update(
        stepper: &mut BevyStepper,
        confirmed: Entity,
        tick: Tick,
    ) {
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .sync_manager
            .duration_since_latest_received_server_tick = Duration::default();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .get_mut::<Confirmed>()
            .unwrap()
            .tick = tick;
    }
}

/// More general integration tests for rollback
#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::rollback::test_utils::received_confirmed_update;
    #[cfg(not(feature = "std"))]
    use alloc::vec::Vec;
    use bevy::ecs::entity::MapEntities;

    use core::time::Duration;
    use serde::{Deserialize, Serialize};

    fn setup(increment_component: bool) -> (BevyStepper, Entity, Entity) {
        fn increment_component_system(
            mut commands: Commands,
            mut query_networked: Query<(Entity, &mut PredictionModeFull), With<Predicted>>,
        ) {
            for (entity, mut component) in query_networked.iter_mut() {
                component.0 += 1.0;
                if component.0 == 5.0 {
                    commands.entity(entity).remove::<PredictionModeFull>();
                }
            }
        }

        let mut stepper = BevyStepper::default();
        if increment_component {
            stepper
                .client_app
                .add_systems(FixedUpdate, increment_component_system);
        }
        // add predicted/confirmed entities
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
        (stepper, confirmed, predicted)
    }

    struct RollbackCounter(pub usize);

    // TODO: check that if A is updated but B is not, and A and B are in the same replication group,
    //  then we need to check the rollback for B as well!
    /// Check that we enter a rollback state when confirmed entity is updated at tick T and:
    /// 1. Predicted component and Confirmed component are different
    /// 2. Confirmed component does not exist and predicted component exists
    /// 3. Confirmed component exists but predicted component does not exist
    /// 4. If confirmed component is the same value as what we have in the history for predicted component, we do not rollback
    #[test]
    fn test_check_rollback() {
        let mut stepper = BevyStepper::default();

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
                PredictionModeFull(1.0),
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
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(PredictionModeFull(1.0));
        // make sure we simulate that we received a server update
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .sync_manager
            .duration_since_latest_received_server_tick = Duration::default();
        stepper.frame_step();
        // 0. Rollback when the Confirmed component is just added
        // (there is a rollback even though the values match, because the value isn't present in
        //  the PredictionHistory at the time of spawn)
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<PredictionMetrics>()
                .rollbacks,
            1
        );

        // 1. Predicted component and confirmed component are different
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(PredictionModeFull(2.0));
        // simulate that we received a server message for the confirmed entity on tick `tick`
        // where the PredictionHistory had the value of 1.0
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<PredictionMetrics>()
                .rollbacks,
            2
        );
        // the predicted history now has PredictionModeFull(2.0)

        // 2. Confirmed component does not exist but predicted component exists
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .remove::<PredictionModeFull>();
        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<PredictionMetrics>()
                .rollbacks,
            3
        );
        // the predicted history now has Absent

        // 3. Confirmed component exists but predicted component does not exist
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .remove::<PredictionModeFull>();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(PredictionModeFull(2.0));
        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<PredictionMetrics>()
                .rollbacks,
            4
        );
        // the predicted history now has ConfirmedSyncModeFull(2.0)

        // 4. If confirmed component is the same value as what we have in the history for predicted component, we do not rollback
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<PredictionModeFull>>()
            .unwrap()
            .add_update(tick, PredictionModeFull(2.0));

        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper.frame_step();
        // no rollback
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<PredictionMetrics>()
                .rollbacks,
            4
        );
    }

    /// Test that the entities within a predicted component marked as to be
    /// entity-mapped are mapped when rollbacked.
    #[test]
    fn test_rollback_entity_mapping() {
        #[derive(Component, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
        struct ComponentWithEntity(Entity);

        impl MapEntities for ComponentWithEntity {
            fn map_entities<M: bevy::prelude::EntityMapper>(&mut self, entity_mapper: &mut M) {
                self.0 = entity_mapper.get_mapped(self.0);
            }
        }

        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let mut stepper = BevyStepper::new(shared_config, ClientConfig::default(), frame_duration);
        // Make `ComponentWithEntity` fully predictable and entity-mappable.
        stepper
            .client_app
            .register_component::<ComponentWithEntity>(ChannelDirection::Bidirectional)
            .add_prediction(PredictionMode::Full)
            .add_map_entities();
        stepper
            .server_app
            .register_component::<ComponentWithEntity>(ChannelDirection::Bidirectional)
            .add_prediction(PredictionMode::Full)
            .add_map_entities();
        stepper.build();
        stepper.init();

        // Spawn a remote entity with a `ComponentWithEntity` component that
        // points to the remote entity. This entity will be replicated to the
        // client and predicted by the client.
        let remote_entity = stepper.server_app.world_mut().spawn_empty().id();
        stepper
            .server_app
            .world_mut()
            .entity_mut(remote_entity)
            .insert((
                ComponentWithEntity(remote_entity),
                crate::server::replication::send::Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        ..default()
                    },
                    ..default()
                },
            ));

        // Wait for server to send replicated component to client.
        for _ in 0..100 {
            stepper.frame_step();
        }

        // Get the confirmed and predicted entities associated with `remote_entity`.
        let confirmed_entity = *stepper
            .client_app
            .world_mut()
            .resource::<ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .remote_to_local
            .get(&remote_entity)
            .unwrap();
        let predicted_entity = *stepper
            .client_app
            .world_mut()
            .resource_mut::<PredictionManager>()
            .predicted_entity_map
            .get_mut()
            .confirmed_to_predicted
            .get(&confirmed_entity)
            .unwrap();

        // Modify `predicted_entity`'s `ComponentWithEntity` to point to some
        // incorrect value, perform a rollback, and verify that
        // `predicted_entity`'s `ComponentWithEntity` points to
        // `predicted_entity`.
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_entity)
            .get_mut::<ComponentWithEntity>()
            .unwrap()
            .0 = Entity::PLACEHOLDER;
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .resource_mut::<Rollback>()
            .set_rollback_tick(tick);
        stepper.tick_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted_entity)
                .get::<ComponentWithEntity>()
                .unwrap()
                .0,
            predicted_entity,
            "Expected predicted component to point to predicted entity"
        );

        // Delete `predicted_entity`'s `ComponentWithEntity`, perform a
        // rollback, and verify that `predicted_entity`'s
        // `ComponentWithEntity` gets re-created and points to
        // `predicted_entity`.
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_entity)
            .remove::<ComponentWithEntity>();
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .resource_mut::<Rollback>()
            .set_rollback_tick(tick);
        stepper.tick_step();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted_entity)
                .get_mut::<ComponentWithEntity>()
                .unwrap()
                .0,
            predicted_entity,
            "Expected predicted component to point to predicted entity"
        );
    }

    /// Test that:
    /// - the `Time` resource's elapsed is rollbacked to the first tick of the rollback
    /// - the `Time` resource's elapsed time is advanced correctly during the rollback
    /// - the `Time` resource's delta during a rollback is the `Time<Fixed>`'s delta
    #[test]
    fn test_rollback_time_resource() {
        #[derive(Debug, PartialEq)]
        struct TimeSnapshot {
            is_rollback: bool,
            delta: Duration,
            elapsed: Duration,
        }

        #[derive(Resource, Default, Debug)]
        struct TimeTracker {
            snapshots: Vec<TimeSnapshot>,
        }

        // Record the time resource's values for each tick.
        fn track_time(
            time: Res<Time>,
            mut time_tracker: ResMut<TimeTracker>,
            rollback: Res<Rollback>,
        ) {
            time_tracker.snapshots.push(TimeSnapshot {
                is_rollback: rollback.is_rollback(),
                delta: time.delta(),
                elapsed: time.elapsed(),
            });
        }

        let (mut stepper, confirmed, predicted) = setup(false);

        // Insert arbitrary predicted component into confirmed. Needed to
        // trigger a rollback.
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(PredictionModeFull(0.0));
        stepper.frame_step();

        // Check that the component got synced.
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(predicted)
                .unwrap(),
            &PredictionModeFull(0.0)
        );

        // Trigger 2 rollback ticks by changing the confirmed's predicted
        // component's value and setting the confirmed's tick to two ticks ago.
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .get_mut::<PredictionModeFull>(confirmed)
            .unwrap()
            .0 = 1.0;
        received_confirmed_update(&mut stepper, confirmed, tick - 2);
        stepper.client_app().insert_resource(TimeTracker::default());
        stepper.client_app().add_systems(FixedUpdate, track_time);

        let time_before_next_tick = *stepper.client_app().world().resource::<Time<Fixed>>();

        stepper.frame_step();

        // Verify that the 2 rollback ticks and regular tick occurred with the
        // correct delta times and elapsed times.
        let time_tracker = stepper.client_app().world().resource::<TimeTracker>();
        assert_eq!(
            time_tracker.snapshots,
            vec![
                TimeSnapshot {
                    is_rollback: true,
                    delta: stepper.tick_duration,
                    elapsed: time_before_next_tick.elapsed() - stepper.tick_duration
                },
                TimeSnapshot {
                    is_rollback: true,
                    delta: stepper.tick_duration,
                    elapsed: time_before_next_tick.elapsed()
                },
                TimeSnapshot {
                    is_rollback: false,
                    delta: stepper.tick_duration,
                    elapsed: time_before_next_tick.elapsed() + stepper.tick_duration
                }
            ]
        );

        // println!("{:?}", stepper.client_app().world().resource::<TimeTracker>());
    }

    /// Test that:
    /// - we remove a component from the predicted entity
    /// - rolling back before the remove should re-add it
    ///   We are still able to rollback properly (the rollback adds the component to the predicted entity)
    #[test]
    fn test_removed_predicted_component_rollback() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let (mut stepper, confirmed, predicted) = setup(true);
        // insert component on confirmed
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(PredictionModeFull(0.0));
        stepper.frame_step();

        // check that the component got synced
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(predicted)
                .unwrap(),
            &PredictionModeFull(1.0)
        );
        // also insert a non-networked component directly on the predicted entity
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentRollback(1.0));

        // advance five more frames, so that the component gets removed on predicted
        for i in 0..5 {
            stepper.frame_step();
        }

        // check that the networked component got removed on predicted
        assert!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(predicted)
                .is_none()
        );
        // also remove the non-networked component
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .remove::<ComponentRollback>();

        // create a rollback situation where the component exists on confirmed but not on predicted
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .get_mut::<PredictionModeFull>(confirmed)
            .unwrap()
            .0 = -10.0;
        received_confirmed_update(&mut stepper, confirmed, tick - 3);
        stepper.frame_step();

        // check that rollback happened
        // predicted got the component re-added and that we rolled back 3 ticks and advances by 1 tick
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .get_mut::<PredictionModeFull>(predicted)
                .unwrap()
                .0,
            -6.0
        );
        // the non-networked component got rolled back
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .get_mut::<ComponentRollback>(predicted)
                .unwrap()
                .0,
            1.0
        );
    }

    /// Test that:
    /// - a component gets added on Predicted
    /// - we trigger a rollback, and the confirmed entity does not have the component
    /// - the rollback removes the component from the predicted entity
    #[test]
    fn test_added_predicted_component_rollback() {
        let (mut stepper, confirmed, predicted) = setup(false);

        // add a new component to Predicted
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(PredictionModeFull(1.0));

        stepper.frame_step();

        // the prediction history is updated with this tick
        let rollback_tick = stepper.client_tick();
        stepper.frame_step();

        // add a non-networked component as well, which should be removed on the rollback
        // since it did not exist at the rollback tick
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentRollback(1.0));

        // create a rollback situation to a tick where
        // - confirmed_component missing
        // - predicted component exists in history
        received_confirmed_update(&mut stepper, confirmed, rollback_tick);
        stepper.frame_step();

        // check that rollback happened:
        // the registered component got removed from predicted since it was not present on confirmed
        assert!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(predicted)
                .is_none()
        );
        // the non-networked component got removed from predicted as it wasn't present in the history
        assert!(
            stepper
                .client_app
                .world()
                .get::<ComponentRollback>(predicted)
                .is_none()
        );
    }

    /// Test that:
    /// - a component gets removed from the Confirmed entity, triggering a rollback
    /// - during the rollback, the component gets removed from the Predicted entity
    #[test]
    fn test_removed_confirmed_component_rollback() {
        let (mut stepper, confirmed, predicted) = setup(true);

        // insert component on confirmed
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(PredictionModeFull(0.0));
        stepper.frame_step();

        // check that the component got synced
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(predicted)
                .unwrap(),
            &PredictionModeFull(1.0)
        );
        // advance a bit more (if we don't then the history contains a component insertion on the first tick,
        // so the rollback will respawn the component)
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();

        // remove the component on confirmed and create a rollback situation
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .remove::<PredictionModeFull>();
        let tick = stepper.client_tick();
        received_confirmed_update(&mut stepper, confirmed, tick - 1);
        stepper.frame_step();

        // check that rollback happened
        // predicted got the component removed
        assert!(
            stepper
                .client_app
                .world_mut()
                .get_mut::<PredictionModeFull>(predicted)
                .is_none()
        );
    }

    /// Test that:
    /// - a component gets added to the confirmed entity, triggering rollback
    /// - the predicted entity did not have the component, so the rollback adds it
    #[test]
    fn test_added_confirmed_component_rollback() {
        let (mut stepper, confirmed, predicted) = setup(true);

        // check that predicted does not have the component
        assert!(
            stepper
                .client_app
                .world_mut()
                .get_mut::<PredictionModeFull>(predicted)
                .is_none()
        );

        // create a rollback situation (confirmed doesn't have a component that predicted has)
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(PredictionModeFull(1.0));
        let tick = stepper.client_tick();
        received_confirmed_update(&mut stepper, confirmed, tick - 2);
        stepper.frame_step();

        // check that rollback happened
        // predicted got the component re-added
        stepper
            .client_app
            .world_mut()
            .get_mut::<PredictionModeFull>(predicted)
            .unwrap()
            .0 = 4.0;
    }

    /// If we have disable_rollback:
    /// 1) we don't check rollback for that entity
    /// 2) if a rollback happens, we reset to the predicted history value instead of the confirmed value
    #[test]
    fn test_disable_rollback() {
        let mut stepper = BevyStepper::default();

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
                PredictionModeFull(1.0),
            ))
            .id();
        let predicted_a = stepper
            .client_app
            .world_mut()
            .spawn((
                Predicted {
                    confirmed_entity: Some(confirmed_a),
                },
                DisableRollback,
            ))
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
                PredictionModeFull(1.0),
            ))
            .id();
        let predicted_b = stepper
            .client_app
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: Some(confirmed_b),
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

        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_a)
            .insert(PredictionModeFull(1000.0));
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_b)
            .insert(PredictionModeFull(1000.0));

        // 1. check rollback doesn't trigger on disable-rollback entities
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed_a)
            .get_mut::<PredictionModeFull>()
            .unwrap()
            .0 = 2.0;
        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed_a, tick);
        let num_rollbacks = stepper
            .client_app
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks;
        stepper.frame_step();
        // no rollback
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<PredictionMetrics>()
                .rollbacks,
            num_rollbacks
        );

        // 2. If a rollback happens, then we reset DisableRollback entities to their historical value
        stepper.frame_step();
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed_b)
            .get_mut::<PredictionModeFull>()
            .unwrap()
            .0 = 3.0;
        let mut history = PredictionHistory::<PredictionModeFull>::default();
        history.add_update(tick, PredictionModeFull(10.0));
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted_a)
            .insert(history);
        // simulate that we received a server message for the confirmed entities on tick `tick`
        // (all predicted entities are in the same ReplicationGroup)
        received_confirmed_update(&mut stepper, confirmed_b, tick);
        received_confirmed_update(&mut stepper, confirmed_a, tick);
        stepper.frame_step();

        // the DisableRollback entity was rolledback to the past PredictionHistory value
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(predicted_a)
                .unwrap()
                .0,
            10.0
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<PredictionModeFull>(predicted_b)
                .unwrap()
                .0,
            3.0
        );
    }
}
