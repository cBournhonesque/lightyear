use std::fmt::Debug;
use std::ops::{Deref, DerefMut};

use bevy::app::FixedMain;
use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::reflect::ReflectResource;
use bevy::prelude::{
    Commands, Component, DespawnRecursiveExt, DetectChanges, Entity, Query, Ref, Res, ResMut,
    Resource, With, Without, World,
};
use bevy::reflect::Reflect;
use bevy::time::{Fixed, Time};
use parking_lot::RwLock;
use tracing::{debug, error, trace, trace_span};

use crate::client::components::{Confirmed, SyncComponent};
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::prediction::correction::Correction;
use crate::client::prediction::diagnostics::PredictionMetrics;
use crate::client::prediction::predicted_history::ComponentState;
use crate::client::prediction::resource::PredictionManager;
use crate::prelude::{ComponentRegistry, PreSpawnedPlayerObject, Tick, TickManager};

use super::predicted_history::PredictionHistory;
use super::resource_history::{ResourceHistory, ResourceState};
use super::Predicted;

/// Resource that indicates whether we are in a rollback state or not
#[derive(Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct Rollback {
    // have to reflect(ignore) this field because of RwLock unfortunately
    #[reflect(ignore)]
    /// We use a RwLock because we want to be able to update this value from multiple systems
    /// in parallel.
    pub state: RwLock<RollbackState>,
    // pub rollback_groups: EntityHashMap<ReplicationGroupId, RollbackState>,
}

/// Resource that will track whether we should do rollback or not
/// (We have this as a resource because if any predicted entity needs to be rolled-back; we should roll back all predicted entities)
#[derive(Debug, Default, Reflect)]
pub enum RollbackState {
    /// We are not in a rollback state
    #[default]
    Default,
    /// We should do a rollback starting from the current_tick
    ShouldRollback {
        /// Current tick of the rollback process
        ///
        /// (note: we will start the rollback from the next tick after we notice the mismatch)
        current_tick: Tick,
    },
}

impl Rollback {
    pub(crate) fn new(state: RollbackState) -> Self {
        Self {
            state: RwLock::new(state),
        }
    }

    /// Returns true if we are currently in a rollback state
    pub fn is_rollback(&self) -> bool {
        match *self.state.read().deref() {
            RollbackState::ShouldRollback { .. } => true,
            RollbackState::Default => false,
        }
    }

    /// Get the current rollback tick
    pub fn get_rollback_tick(&self) -> Option<Tick> {
        match *self.state.read().deref() {
            RollbackState::ShouldRollback { current_tick } => Some(current_tick),
            RollbackState::Default => None,
        }
    }

    /// Increment the rollback tick
    pub(crate) fn increment_rollback_tick(&self) {
        if let RollbackState::ShouldRollback {
            ref mut current_tick,
        } = *self.state.write().deref_mut()
        {
            *current_tick += 1;
        }
    }

    /// Set the rollback state back to non-rollback
    pub(crate) fn set_non_rollback(&self) {
        *self.state.write().deref_mut() = RollbackState::Default;
    }

    /// Set the rollback state to `ShouldRollback` with the given tick
    pub(crate) fn set_rollback_tick(&self, tick: Tick) {
        *self.state.write().deref_mut() = RollbackState::ShouldRollback { current_tick: tick };
    }
}

/// Check if we need to do a rollback.
/// We do this separately from `prepare_rollback` because even if component A is the same between predicted and confirmed,
/// if component B is different we do a rollback for all components
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_rollback<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    // TODO: have a way to only get the updates of entities that are predicted?
    tick_manager: Res<TickManager>,
    connection: Res<ConnectionManager>,
    // We also snap the value of the component to the server state if we are in rollback
    mut predicted_query: Query<&mut PredictionHistory<C>, (With<Predicted>, Without<Confirmed>)>,
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    confirmed_query: Query<(Entity, Option<&C>, Ref<Confirmed>)>,
    rollback: Res<Rollback>,
) {
    // TODO: can just enable bevy spans?
    let _span = trace_span!("client rollback check");
    let kind = std::any::type_name::<C>();

    // TODO: for mode=simple/once, we still need to re-add the component if the entity ends up not being despawned!

    // TODO: maybe we can check if we receive any replication packets?
    // no need to check for rollback if we didn't receive any packet
    if !connection.received_new_server_tick() {
        return;
    }

    let current_tick = tick_manager.tick();
    for (confirmed_entity, confirmed_component, confirmed) in confirmed_query.iter() {
        // NOTE: it is not enough to check if we received any ComponentRemoveEvent<C>, ComponentUpdateEvent<C> and ComponentInsertEvent<C>
        //  because we could have entity A and B in the same ReplicationGroup.
        //  We receive a message with entity B updates, but no entity A updates, **which means that entity A is still in the same state as before**
        //  on the confirmed tick! This means that we received an update for entity A, and we still need to check for rollback.

        // TODO: using `!confirmed.is_changed` is a potential bug! we only want a rollback check to trigger when the confirmed tick is updated!
        //  but not when the confirmed entity is first spawned, no? when the entity is first spawn, currently
        //  we still do a rollback check immediately
        //  instead use `!confirmed.is_changed() || confirmed.is_added() { continue; }`?
        //  but figure out how to adapt tests

        // 0. only check rollback when any entity in the replication group has been updated
        // (i.e. the confirmed tick has been updated)
        if !confirmed.is_changed() {
            continue;
        }

        // 1. Get the predicted entity, and it's history
        let Some(p) = confirmed.predicted else {
            continue;
        };
        let Ok(mut predicted_history) = predicted_query.get_mut(p) else {
            debug!(
                "Predicted entity {:?} was not found when checking rollback for {:?}",
                confirmed.predicted,
                std::any::type_name::<C>()
            );
            continue;
        };

        // 2. We will compare the predicted history and the confirmed entity at the current confirmed entity tick
        // - Confirmed contains the server state at the tick
        // - History contains the history of what we predicted at the tick
        // get the tick that the confirmed entity is at
        let tick = confirmed.tick;
        if tick > current_tick {
            debug!(
                "Confirmed entity {:?} is at a tick in the future: {:?} compared to client timeline. Current tick: {:?}",
                confirmed_entity,
                tick,
                current_tick
            );
            continue;
        }

        // 3.a We are still not sure if we should do rollback. Compare history against confirmed
        // We rollback if there's no history (newly added predicted entity, or if there is a mismatch)
        if !rollback.is_rollback() {
            let history_value = predicted_history.pop_until_tick(tick);
            let predicted_exist = history_value.is_some();
            let confirmed_exist = confirmed_component.is_some();
            let should_rollback = match confirmed_component {
                // TODO: history-value should not be empty here; should we panic if it is?
                // confirm does not exist. rollback if history value is not Removed
                None => history_value.map_or(false, |history_value| {
                    history_value != ComponentState::Removed
                }),
                // confirm exist. rollback if history value is different
                Some(c) => history_value.map_or(true, |history_value| match history_value {
                    ComponentState::Updated(history_value) => {
                        component_registry.should_rollback(&history_value, c)
                    }
                    ComponentState::Removed => true,
                }),
            };
            if should_rollback {
                debug!(
                   ?predicted_exist, ?confirmed_exist,
                   "Rollback check: mismatch for component between predicted and confirmed {:?} on tick {:?} for component {:?}. Current tick: {:?}",
                   confirmed_entity, tick, kind, current_tick
                   );
                // we already rolled-back the state for the entity's latest_tick
                // after this we will start right away with a physics update, so we need to start taking the inputs from the next tick
                rollback.set_rollback_tick(tick + 1);
            }
        } else {
            // 3.b We already know we should do rollback (because of another entity/component), start the rollback
            trace!(
                   "Rollback check: should roll back for component between predicted and confirmed on tick {:?} for component {:?}. Current tick: {:?}",
                   tick, kind, current_tick
                   );
        }
    }
}

/// If there is a mismatch, prepare rollback for all components
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (
            Option<&mut C>,
            &mut PredictionHistory<C>,
            Option<&mut Correction<C>>,
        ),
        (
            With<Predicted>,
            Without<Confirmed>,
            Without<PreSpawnedPlayerObject>,
        ),
    >,
    confirmed_query: Query<(Entity, Option<&C>, Ref<Confirmed>)>,
    rollback: Res<Rollback>,
    manager: Res<PredictionManager>,
) {
    let kind = std::any::type_name::<C>();

    let _span = trace_span!("client rollback prepare");
    debug!("in prepare rollback");

    let current_tick = tick_manager.tick();
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

        // 1. Get the predicted entity, and it's history
        let Ok((predicted_component, mut predicted_history, mut correction)) =
            predicted_query.get_mut(predicted_entity)
        else {
            debug!(
                "Predicted entity {:?} was not found when preparing rollback for {:?}",
                confirmed.predicted,
                std::any::type_name::<C>()
            );
            continue;
        };

        // 2. we need to clear the history so we can write a new one
        predicted_history.clear();
        // SAFETY: we know the predicted entity exists
        let mut entity_mut = commands.entity(predicted_entity);

        // 3. we update the state to the Corrected state
        // NOTE: visually, we will use the CorrectionFn to interpolate between the current Predicted state and the Corrected state
        //  even though for other purposes (physics, etc.) we switch directly to the Corrected state
        match confirmed_component {
            // confirm does not exist, remove on predicted
            None => {
                predicted_history
                    .buffer
                    .push(rollback_tick, ComponentState::Removed);
                entity_mut.remove::<C>();
            }
            // confirm exist, update or insert on predicted
            Some(confirmed_component) => {
                let mut rollbacked_predicted_component = confirmed_component.clone();
                let _ = manager.map_entities(
                    &mut rollbacked_predicted_component,
                    component_registry.as_ref(),
                );
                predicted_history.buffer.push(
                    rollback_tick,
                    ComponentState::Updated(rollbacked_predicted_component.clone()),
                );
                match predicted_component {
                    None => {
                        debug!("Re-adding deleted Full component to predicted");
                        entity_mut.insert(rollbacked_predicted_component.clone());
                    }
                    Some(mut predicted_component) => {
                        // // no need to do a correction if the values are the same
                        // if predicted_component.as_ref() == c {
                        //     continue;
                        // }

                        // insert the Correction information only if the component exists on both confirmed and predicted
                        let correction_ticks = ((current_tick - rollback_tick) as f32
                            * config.prediction.correction_ticks_factor)
                            .round() as i16;

                        // no need to add the Correction if the correction is instant
                        if correction_ticks != 0 && component_registry.has_correction::<C>() {
                            let final_correction_tick = current_tick + correction_ticks;
                            if let Some(correction) = correction.as_mut() {
                                debug!("updating existing correction");
                                // if there is a correction, start the correction again from the previous
                                // visual state to avoid glitches
                                correction.original_prediction =
                                    std::mem::take(&mut correction.current_visual)
                                        .unwrap_or_else(|| predicted_component.clone());
                                correction.original_tick = current_tick;
                                correction.final_correction_tick = final_correction_tick;
                                // TODO: can set this to None, shouldnt make any diff
                                correction.current_correction =
                                    Some(rollbacked_predicted_component.clone());
                            } else {
                                debug!("inserting new correction");
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
                        *predicted_component = rollbacked_predicted_component.clone();
                    }
                };
            }
        };
    }
}

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
    tick_manager: Res<TickManager>,
    // TODO: have a way to make these systems run in parallel
    //  - either by using RwLock in PredictionManager
    //  - or by using a system that iterates through archetypes, a la replicon?
    mut prediction_manager: ResMut<PredictionManager>,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (Entity, Option<&mut C>, &mut PredictionHistory<C>),
        (
            With<PreSpawnedPlayerObject>,
            Without<Confirmed>,
            Without<Predicted>,
        ),
    >,
    rollback: Res<Rollback>,
) {
    let kind = std::any::type_name::<C>();
    let _span = trace_span!("client prepare rollback for pre-spawned entities");

    let current_tick = tick_manager.tick();

    let Some(rollback_tick_plus_one) = rollback.get_rollback_tick() else {
        error!("prepare_rollback_prespawn should only be called when we are in rollback");
        return;
    };

    // careful, the current_tick is already incremented by 1 in the check_rollback stage...
    let rollback_tick = rollback_tick_plus_one - 1;

    // 0. If the prespawned entity didn't exist at the rollback tick, despawn it
    // TODO: also handle deleting pre-predicted entities!
    // NOTE: if rollback happened at current_tick - 1, then we will start running systems starting from current_tick.
    //  so if the entity was spawned at tick >= current_tick, we despawn it, and it can get respawned again
    let mut entities_to_despawn = EntityHashSet::default();
    for (_, hash) in prediction_manager
        .prespawn_tick_to_hash
        .drain_after(&rollback_tick_plus_one)
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
        if let Some(entity_commands) = commands.get_entity(*entity) {
            entity_commands.despawn_recursive();
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
            None | Some(ComponentState::Removed) => {
                if predicted_component.is_some() {
                    debug!(?prespawned_entity, ?kind, "Component for prespawned entity didn't exist at time of rollback, removing it");
                    // the component didn't exist at the time, remove it!
                    commands.entity(prespawned_entity).remove::<C>();
                }
            }
            Some(ComponentState::Updated(c)) => {
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
pub(crate) fn prepare_rollback_non_networked<C: Component + PartialEq + Clone>(
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (Entity, Option<&mut C>, &mut PredictionHistory<C>),
        With<Predicted>,
    >,
    rollback: Res<Rollback>,
) {
    let kind = std::any::type_name::<C>();
    let _span = trace_span!("client prepare rollback for non networked component", ?kind);

    let current_tick = tick_manager.tick();
    let Some(rollback_tick_plus_one) = rollback.get_rollback_tick() else {
        error!("prepare_rollback_non_networked_components should only be called when we are in rollback");
        return;
    };

    // careful, the current_tick is already incremented by 1 in the check_rollback stage...
    let rollback_tick = rollback_tick_plus_one - 1;

    // 0. If the entity didn't exist at the rollback tick, despawn it
    // TODO? or is it handled for us?

    for (entity, component, mut history) in predicted_query.iter_mut() {
        // 1. restore the component to the historical value
        match history.pop_until_tick(rollback_tick) {
            None | Some(ComponentState::Removed) => {
                if component.is_some() {
                    debug!(?entity, ?kind, "Non-networked component for predicted entity didn't exist at time of rollback, removing it");
                    // the component didn't exist at the time, remove it!
                    commands.entity(entity).remove::<C>();
                }
            }
            Some(ComponentState::Updated(c)) => {
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
    tick_manager: Res<TickManager>,
    rollback: Res<Rollback>,
    resource: Option<ResMut<R>>,
    mut history: ResMut<ResourceHistory<R>>,
) {
    let kind = std::any::type_name::<R>();
    let _span = trace_span!("client prepare rollback for resource", ?kind);

    let current_tick = tick_manager.tick();
    let Some(rollback_tick_plus_one) = rollback.get_rollback_tick() else {
        error!("prepare_rollback_resource should only be called when we are in rollback");
        return;
    };

    // careful, the current_tick is already incremented by 1 in the check_rollback stage...
    let rollback_tick = rollback_tick_plus_one - 1;

    // 1. restore the resource to the historical value
    match history.pop_until_tick(rollback_tick) {
        None | Some(ResourceState::Removed) => {
            if resource.is_some() {
                debug!(
                    ?kind,
                    "Resource didn't exist at time of rollback, removing it"
                );
                // the resource didn't exist at the time, remove it!
                commands.remove_resource::<R>();
            }
        }
        Some(ResourceState::Updated(r)) => {
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

    let rollback_elapsed_time = current_fixed_time.elapsed() - rollback_time_offset;
    rollback_fixed_time.advance_to(rollback_elapsed_time - rollback_fixed_time.timestep());
    // Time<Fixed>::delta is set to the value provided in `advance_by` (or
    // `advance_to`), so we want to call
    // `advance_by(rollback_fixed_time.timestep())` at the end to set the delta
    // value that is expected.
    rollback_fixed_time.advance_by(rollback_fixed_time.timestep());

    rollback_fixed_time
}

pub(crate) fn run_rollback(world: &mut World) {
    let tick_manager = world.get_resource::<TickManager>().unwrap();
    let rollback = world.get_resource::<Rollback>().unwrap();
    let current_tick = tick_manager.tick();

    // NOTE: all predicted entities should be on the same tick!
    // TODO: might not need to check the state, because we only run this system if we are in rollback
    let current_rollback_tick = rollback
        .get_rollback_tick()
        .expect("we should be in rollback");

    // NOTE: careful! we restored the state to the end of tick `confirmed` = `current_rollback_tick - 1`
    //  we want to run fixed-update to be at the end of `current_tick`, so we need to run
    // `current_tick - (current_rollback_tick - 1)` ticks
    // (we set `current_rollback_tick` to `confirmed + 1` so that on the FixedUpdate rollback run, we fetch the input for
    // `confirmed + 1`
    let num_rollback_ticks = current_tick + 1 - current_rollback_tick;
    debug!(
        "Rollback between {:?} and {:?}",
        current_rollback_tick, current_tick
    );

    // Keep track of the generic time resource so it can be restored after the
    // rollback.
    let time_resource = *world.resource::<Time>();

    // Rollback the fixed time resource in preparation for the rollback.
    let current_fixed_time = *world.resource::<Time<Fixed>>();
    *world.resource_mut::<Time<Fixed>>() =
        rollback_fixed_time(&current_fixed_time, num_rollback_ticks);

    // Run the fixed update schedule (which should contain ALL
    // predicted/rollback components and resources). This is similar to what
    // `bevy_time::fixed::run_fixed_main_schedule()` does
    for i in 0..num_rollback_ticks {
        debug!("Rollback tick: {:?}", current_rollback_tick + i);

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
    let rollback = world.get_resource_mut::<Rollback>().unwrap();
    rollback.set_non_rollback();
}

pub(crate) fn increment_rollback_tick(rollback: Res<Rollback>) {
    trace!("increment rollback tick");
    rollback.increment_rollback_tick();
}

#[cfg(test)]
pub(super) mod test_utils {
    use crate::client::components::Confirmed;
    use crate::client::connection::ConnectionManager;
    use crate::prelude::Tick;
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::Entity;
    use std::time::Duration;

    /// Helper function to simulate that we received a server message
    pub(super) fn received_confirmed_update(
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

#[cfg(test)]
mod unit_tests {
    use super::test_utils::*;
    use super::*;

    use crate::tests::protocol::ComponentSyncModeFull;
    use crate::tests::stepper::BevyStepper;
    use bevy::ecs::system::RunSystemOnce;

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
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeFull(1.0));
        stepper.frame_step();

        // 1. Predicted component and confirmed component are different
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .get_mut::<ComponentSyncModeFull>()
            .unwrap()
            .0 = 2.0;
        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper
            .client_app
            .world_mut()
            .run_system_once(check_rollback::<ComponentSyncModeFull>);
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<Rollback>()
                .get_rollback_tick(),
            Some(tick + 1)
        );

        // 2. Confirmed component does not exist but predicted component exists
        // reset rollback state
        stepper
            .client_app
            .world()
            .resource::<Rollback>()
            .set_non_rollback();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .remove::<ComponentSyncModeFull>();
        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper
            .client_app
            .world_mut()
            .run_system_once(check_rollback::<ComponentSyncModeFull>);
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<Rollback>()
                .get_rollback_tick(),
            Some(tick + 1)
        );

        // 3. Confirmed component exists but predicted component does not exist
        // reset rollback state
        stepper
            .client_app
            .world()
            .resource::<Rollback>()
            .set_non_rollback();
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .remove::<ComponentSyncModeFull>();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeFull(2.0));
        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper
            .client_app
            .world_mut()
            .run_system_once(check_rollback::<ComponentSyncModeFull>);
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<Rollback>()
                .get_rollback_tick(),
            Some(tick + 1)
        );

        // 4. If confirmed component is the same value as what we have in the history for predicted component, we do not rollback
        // reset rollback state
        stepper
            .client_app
            .world_mut()
            .resource::<Rollback>()
            .set_non_rollback();
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<ComponentSyncModeFull>>()
            .unwrap()
            .add_update(tick, ComponentSyncModeFull(2.0));
        // simulate that we received a server message for the confirmed entity on tick `tick`
        received_confirmed_update(&mut stepper, confirmed, tick);
        stepper
            .client_app
            .world_mut()
            .run_system_once(check_rollback::<ComponentSyncModeFull>);
        assert!(!stepper
            .client_app
            .world()
            .resource::<Rollback>()
            .is_rollback());
    }
}

/// More general integration tests for rollback
#[cfg(test)]
mod integration_tests {
    use std::time::Duration;

    use super::test_utils::*;

    use crate::client::prediction::resource::PredictionManager;
    use crate::prelude::server::SyncTarget;
    use crate::prelude::{
        client::*, AppComponentExt, ChannelDirection, NetworkTarget, SharedConfig, TickConfig,
    };
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use bevy::ecs::entity::MapEntities;
    use bevy::prelude::*;
    use serde::{Deserialize, Serialize};

    fn setup(increment_component: bool) -> (BevyStepper, Entity, Entity) {
        fn increment_component_system(
            mut commands: Commands,
            mut query_networked: Query<(Entity, &mut ComponentSyncModeFull), With<Predicted>>,
        ) {
            for (entity, mut component) in query_networked.iter_mut() {
                component.0 += 1.0;
                if component.0 == 5.0 {
                    commands.entity(entity).remove::<ComponentSyncModeFull>();
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

    /// Test that the entities within a predicted component marked as to be
    /// entity-mapped are mapped when rollbacked.
    #[test]
    fn test_rollback_entity_mapping() {
        #[derive(Component, Serialize, Deserialize, Clone, Copy, PartialEq)]
        struct ComponentWithEntity(Entity);

        impl MapEntities for ComponentWithEntity {
            fn map_entities<M: bevy::prelude::EntityMapper>(&mut self, entity_mapper: &mut M) {
                self.0 = entity_mapper.map_entity(self.0);
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
            .add_prediction(ComponentSyncMode::Full)
            .add_map_entities();
        stepper
            .server_app
            .register_component::<ComponentWithEntity>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full)
            .add_map_entities();
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
            .insert(ComponentSyncModeFull(0.0));
        stepper.frame_step();

        // Check that the component got synced.
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(predicted)
                .unwrap(),
            &ComponentSyncModeFull(0.0)
        );

        // Trigger 2 rollback ticks by changing the confirmed's predicted
        // component's value and setting the confirmed's tick to two ticks ago.
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(confirmed)
            .unwrap()
            .0 = 1.0;
        received_confirmed_update(&mut stepper, confirmed, tick - 2);
        stepper.client_app.insert_resource(TimeTracker::default());
        stepper.client_app.add_systems(FixedUpdate, track_time);

        let time_before_next_tick = *stepper.client_app.world().resource::<Time<Fixed>>();

        stepper.frame_step();

        // Verify that the 2 rollback ticks and regular tick occurred with the
        // correct delta times and elapsed times.
        let time_tracker = stepper.client_app.world().resource::<TimeTracker>();
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

        println!("{:?}", stepper.client_app.world().resource::<TimeTracker>());
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
            .insert(ComponentSyncModeFull(0.0));
        stepper.frame_step();

        // check that the component got synced
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(predicted)
                .unwrap(),
            &ComponentSyncModeFull(1.0)
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
        assert!(stepper
            .client_app
            .world()
            .get::<ComponentSyncModeFull>(predicted)
            .is_none());
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
            .get_mut::<ComponentSyncModeFull>(confirmed)
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
                .get_mut::<ComponentSyncModeFull>(predicted)
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
        let (mut stepper, confirmed, predicted) = setup(true);

        // add a new component to Predicted
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentSyncModeFull(1.0));
        stepper.frame_step();

        // create a rollback situation (confirmed doesn't have a component that predicted has)
        let tick = stepper.client_tick();
        received_confirmed_update(&mut stepper, confirmed, tick - 1);

        // add a non-networked component as well, which should be removed on the rollback
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentRollback(1.0));
        stepper.frame_step();

        // check that rollback happened: the component got removed from predicted
        assert!(stepper
            .client_app
            .world()
            .get::<ComponentSyncModeFull>(predicted)
            .is_none());
        assert!(stepper
            .client_app
            .world()
            .get::<ComponentRollback>(predicted)
            .is_none());
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
            .insert(ComponentSyncModeFull(0.0));
        stepper.frame_step();

        // check that the component got synced
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(predicted)
                .unwrap(),
            &ComponentSyncModeFull(1.0)
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
            .remove::<ComponentSyncModeFull>();
        let tick = stepper.client_tick();
        received_confirmed_update(&mut stepper, confirmed, tick - 1);
        stepper.frame_step();

        // check that rollback happened
        // predicted got the component removed
        assert!(stepper
            .client_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(predicted)
            .is_none());
    }

    /// Test that:
    /// - a component gets added to the confirmed entity, triggering rollback
    /// - the predicted entity did not have the component, so the rollback adds it
    #[test]
    fn test_added_confirmed_component_rollback() {
        let (mut stepper, confirmed, predicted) = setup(true);

        // check that predicted does not have the component
        assert!(stepper
            .client_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(predicted)
            .is_none());

        // create a rollback situation (confirmed doesn't have a component that predicted has)
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeFull(1.0));
        let tick = stepper.client_tick();
        received_confirmed_update(&mut stepper, confirmed, tick - 2);
        stepper.frame_step();

        // check that rollback happened
        // predicted got the component re-added
        stepper
            .client_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(predicted)
            .unwrap()
            .0 = 4.0;
    }
}
