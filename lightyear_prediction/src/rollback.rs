use super::predicted_history::PredictionHistory;
use super::resource_history::ResourceHistory;
use super::{Predicted, SyncComponent};
use crate::correction::PreviousVisual;
use crate::despawn::PredictionDisable;
use crate::diagnostics::PredictionMetrics;
use crate::manager::{LastConfirmedInput, PredictionManager, PredictionResource, RollbackMode, StateRollbackMetadata};
use crate::plugin::PredictionSystems;
use crate::registry::PredictionRegistry;
use alloc::vec::Vec;
use bevy_app::FixedMain;
use bevy_app::prelude::*;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::ScheduleLabel;
use bevy_ecs::system::{ParamBuilder, QueryParamBuilder};
use bevy_ecs::world::{DeferredWorld, FilteredEntityMut};
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time};
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use bevy_replicon::prelude::{ClientMessages, ClientSystems};
use bevy_replicon::shared::backend::channels::ServerChannel;
use lightyear_connection::host::HostClient;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{Rollback, is_in_rollback};
use lightyear_frame_interpolation::FrameInterpolationSystems;
use lightyear_replication::prelude::{ConfirmHistory};
use lightyear_replication::prespawn::{PreSpawned, PreSpawnedReceiver};
use lightyear_replication::registry::ComponentRegistry;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
#[cfg(feature = "metrics")]
use lightyear_utils::metrics::TimerGauge;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{debug, debug_span, error, info, trace, trace_span, warn};

/// Responsible for re-running the FixedMain schedule a fixed number of times in order
/// to rollback the simulation to a previous state.
#[derive(Debug, Hash, PartialEq, Eq, Clone, ScheduleLabel)]
pub struct RollbackSchedule;

#[deprecated(note = "Use RollbackSystems instead")]
pub type RollbackSet = RollbackSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RollbackSystems {
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
        // SETS
        app.configure_sets(
            PreUpdate,
            (
                RollbackSystems::Check,
                RollbackSystems::RemoveDisable.run_if(is_in_rollback),
                RollbackSystems::Prepare.run_if(is_in_rollback),
                RollbackSystems::Rollback.run_if(is_in_rollback),
                RollbackSystems::EndRollback.run_if(is_in_rollback),
            )
                .chain()
                .in_set(PredictionSystems::Rollback),
        );
        app.configure_sets(
            PostUpdate,
            // we add the correction error AFTER the interpolation was done
            // (which means it's also after we buffer the component for replication)
            RollbackSystems::VisualCorrection
                .after(FrameInterpolationSystems::Interpolate)
                .in_set(PredictionSystems::All),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            (
                check_received_replication_messages
                    .after(ClientSystems::ReceivePackets)
                    .before(ClientSystems::Receive),
                reset_input_rollback_tracker.after(RollbackSystems::Check),
                remove_prediction_disable.in_set(RollbackSystems::RemoveDisable),
                run_rollback.in_set(RollbackSystems::Rollback),
                end_rollback.in_set(RollbackSystems::EndRollback),
                #[cfg(feature = "metrics")]
                no_rollback
                    .after(RollbackSystems::Check)
                    .in_set(PredictionSystems::All)
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
                builder.data::<&Predicted>();
                builder.data::<&ConfirmHistory>();
                builder.without::<DeterministicPredicted>();
                builder.without::<DisableRollback>();
                // include PredictionDisable entities (entities that are predicted and 'despawned'
                // but we keep them around for rollback check)
                builder.filter::<Allow<PredictionDisable>>();
                builder.optional(|b| {
                    // include access to &mut PredictionHistory<C> and &Confirmed<C> for all predicted components, if the components are replicated
                    // (no need to check rollback for non-networked components)
                    prediction_registry
                        .prediction_map
                        .iter()
                        // don't check_rollback for non-networked components, which are not present in the ComponentRegistry
                        .filter_map(|(kind, p)| {
                            component_registry
                                .component_metadata_map
                                .get(kind)
                                .map(|c| (c, p))
                        })
                        .for_each(|(m, p)| {
                            b.mut_id(p.history_id);
                        });
                });
            }),
            ParamBuilder,
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

        app.add_systems(PreUpdate, check_rollback.in_set(RollbackSystems::Check));

        app.insert_resource(component_registry);
        app.insert_resource(prediction_registry);
    }
}

#[derive(Component, PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
#[component(on_add = DeterministicPredicted::on_add)]
/// Marker component used to indicate this entity is predicted (it has a PredictionHistory),
/// but it won't check for rollback from state updates.
///
/// This can be used to mark predicted non-networked entities in deterministic replication, or to stop a
/// state-replicated entity from being able to trigger rollbacks from state mismatch.
///
/// This entity will still get rolled back to its predicted history when a rollback happens.
pub struct DeterministicPredicted {
    /// After spawning a DeterministicPredicted entity, any rollback that happens shortly after might
    /// despawn the entity (since it didn't exist at the start of rollback) or remove its components.
    ///
    /// If the entity was spawned in a deterministic manner (for instance with a 'Shoot' input), then we
    /// want the entity to be despawned as it will get re-created during rollback.
    /// But if the entity was spawned as a one-off event (for example replicated by the server upon connection),
    /// we don't want the entity to be affected by rollbacks for a short period after being spawned.
    pub skip_despawn: bool,
    /// For entities where skip_despawn is True, after how many ticks do we start enabling back rollbacks?
    pub enable_rollback_after: u8,
}

impl Default for DeterministicPredicted {
    fn default() -> Self {
        Self {
            skip_despawn: false,
            enable_rollback_after: 20,
        }
    }
}

impl DeterministicPredicted {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        // TODO: avoid fetching DeterministicPredicted twice when we can convert DeferredWorld to UnsafeWorldCell (0.17.3)
        let deterministic_predicted = *world.get::<DeterministicPredicted>(context.entity).unwrap();
        let tick = world.resource::<LocalTimeline>().tick();
        let Some(prediction_manager_entity) = world
            .get_resource::<PredictionResource>()
            .map(|r| r.link_entity)
        else {
            return;
        };
        let Some(mut manager) = world.get_mut::<PredictionManager>(prediction_manager_entity)
        else {
            return;
        };
        if !deterministic_predicted.skip_despawn {
            manager.deterministic_despawn.push((tick, context.entity));
        } else {
            info!(entity = ?context.entity, "ADDING SKIP DESPAWN");
            manager.deterministic_skip_despawn.push((
                tick + (deterministic_predicted.enable_rollback_after as i16),
                context.entity,
            ));
        }
    }
}

/// Marker component to indicate that the entity will be completely excluded from rollbacks.
/// It won't be part of rollback checks, and it won't be rolled back to a past state if a rollback happens.
#[derive(Component, Debug)]
pub struct DisableRollback;

#[derive(Component)]
/// Marker `Disabled` component inserted on `DisableRollback` entities during rollbacks so
/// that they are ignored from all queries
pub struct DisabledDuringRollback;


/// Set a flag if we received any replication message this frame
fn check_received_replication_messages(
    client_messages: Res<ClientMessages>,
    mut metadata: ResMut<StateRollbackMetadata>
) {
    metadata.received_messages_this_frame = false;
    if client_messages.received_count(ServerChannel::Updates) > 0 || client_messages.received_count(ServerChannel::Mutations) > 0 {
        metadata.received_messages_this_frame = true;
    }
}

/// Check if we need to do a rollback.
/// We do this separately from `prepare_rollback` because even we stop the `check_rollback` function
/// early as soon as we find a mismatch, but we need to rollback all components to the original state.
fn check_rollback(
    // we want Query<(&mut PredictionHistory<C>, &Confirmed<C>), With<Predicted>>
    // make sure to include disabled entities
    mut predicted_entities: Query<FilteredEntityMut>,
    timeline: Res<LocalTimeline>,
    state_metadata: Res<StateRollbackMetadata>,
    receiver_query: Single<
        (
            Entity,
            Option<&LastConfirmedInput>,
            &mut PredictionManager,
            &mut PreSpawnedReceiver,
        ),
        (With<IsSynced<InputTimeline>>, Without<HostClient>),
    >,
    component_registry: Res<ComponentRegistry>,
    prediction_registry: Res<PredictionRegistry>,
    parallel_commands: ParallelCommands,
    mut commands: Commands,
) {
    #[cfg(feature = "metrics")]
    let _timer = TimerGauge::new("prediction/rollback/check");

    // TODO: iterate through each archetype in parallel? using rayon

    // TODO: maybe have a sparse-set component with ConfirmedUpdated to quickly query only through predicted entities
    //  that received a confirmed update? Would the iteration even be faster? since entities with or without sparse-set
    //  would still be in the same table
    let (
        manager_entity,
        last_confirmed_input,
        mut prediction_manager,
        mut prespawned_receiver,
    ) = receiver_query.into_inner();
    let tick = timeline.tick();
    let received_state = state_metadata.received_messages_this_frame;

    let do_rollback = move |rollback_tick: Tick,
                            prediction_manager: &PredictionManager,
                            commands: &mut Commands,
                            rollback: Rollback| {
        let max_rollback_ticks = prediction_manager.rollback_policy.max_rollback_ticks;
        let delta = tick - rollback_tick;
        if delta < 0 || delta as u16 > max_rollback_ticks {
            warn!(
                ?rollback_tick,
                ?tick,
                "Trying to do a rollback of {delta:?} ticks. The max is {max_rollback_ticks:?}! Aborting"
            );
            prediction_manager.set_non_rollback();
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
            if received_state && !predicted_entities.is_empty() {
                // TODO: only valid if all predicted entities are in the same group
                debug!(
                    "Rollback because we have received a new confirmed state. (no mismatch check)"
                );
                do_rollback(
                    state_metadata.last_confirmed_tick(),
                    &prediction_manager,
                    &mut commands,
                    Rollback::FromState,
                );
            };
        }
        RollbackMode::Check => {
            // maybe we know we should rollback after there was a mismatch on receiving a confirmed update
            if state_metadata.should_rollback {
                do_rollback(
                    state_metadata.last_confirmed_tick(),
                    &prediction_manager,
                    &mut commands,
                    Rollback::FromState,
                );
            }
            // no need to check for rollback if we didn't receive any state this frame
            if received_state {
                let last_confirmed_tick: Tick = state_metadata.last_confirmed_tick();
                if last_confirmed_tick > tick {
                    debug!(
                        "LastConfirmedTick tick is in the future: {:?} compared to client timeline. Current tick: {:?}",
                        last_confirmed_tick,
                        tick
                    );
                    return;
                }

                // we already check for rollbacks when we receive mutations on predicted entities
                // here we want to check if we need to rollback because of mispredictions when we **didn't** receive a mutation
                // (i.e. ServerMutateTick > ConfirmHistory)

                trace!(?tick, "Checking for state-based rollback");
                predicted_entities.par_iter_mut().for_each(|mut entity_mut| {
                    // we don't end early if we find a rollback, since we will need to
                    // TODO: should we introduce a Rollback marker component?
                    // // we already know we are in rollback, no need to check again
                    if prediction_manager.is_rollback() {
                        return
                    }

                    let confirm_history_tick: Tick = entity_mut.get::<ConfirmHistory>().unwrap().last_tick().get().into();
                    if confirm_history_tick >= last_confirmed_tick {
                        return
                    }

                    trace!("Checking rollback for entity {:?} where we know that the component was not updated from last confirmed tick {confirm_history_tick:?} to new confirmed tick {last_confirmed_tick:?}", entity_mut.id());

                    // TODO: maybe pre-cache the components of the archetypes that we want to iterate over?
                    //  we need to archetypes that have Predicted, and we cache the history id and the confirmed id. (The confirmed could be absent)
                    for check_rollback in prediction_registry.prediction_map
                        .iter()
                        .filter_map(|(kind, p)|
                            // TODO: add filter to only look at networked components!
                            // only check rollback for components that are replicated (ignore non-networked)
                            component_registry.component_metadata_map.contains_key(kind).then_some(p.check_rollback)
                        )
                        .take_while(|_| !prediction_manager.is_rollback())
                    {
                        if check_rollback(&prediction_registry, last_confirmed_tick, &mut entity_mut) {
                            debug!("Rollback because we have received a new confirmed state. (mismatch check)");
                            // During `prepare_rollback` we will reset the component to their values on `confirmed_tick`.
                            // Then when we do Rollback in PreUpdate, we will start by incrementing the tick, which will be equal to `confirmed_tick + 1`
                            parallel_commands.command_scope(|mut c| {
                                // if there is a rollback, we start from the last_confirmed_tick! (earliest confirmed tick among
                                // all predicted entities)
                                do_rollback(last_confirmed_tick, &prediction_manager, &mut c, Rollback::FromState);
                            });
                            return;
                        }
                    }
                });
            }
        }
        // set rollback from the LastConfirmedInput
        RollbackMode::Disabled => {}
    }

    // if we don't have state-based rollbacks, check for input-rollbacks
    match prediction_manager.rollback_policy.input {
        // If we have received any input message, rollback from the last confirmed input
        RollbackMode::Always => {
            if prediction_manager.is_rollback() {
                debug!("Rollback was triggered by state, skipping input rollback checks");
            } else {
                if let Some(last_confirmed_input) = last_confirmed_input
                    && last_confirmed_input.received_input()
                {
                    debug!(
                        ?last_confirmed_input,
                        "Rollback because we have received a new remote input. (no mismatch check)"
                    );
                    let rollback_tick = last_confirmed_input.tick.get();
                    do_rollback(
                        rollback_tick,
                        &prediction_manager,
                        &mut commands,
                        Rollback::FromInputs,
                    );
                }
            }
        }
        // Rollback from any mismatched input
        RollbackMode::Check => {
            if prediction_manager.is_rollback() {
                debug!("Rollback was triggered by state, skipping input rollback checks");
            } else {
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
        }
        RollbackMode::Disabled => {}
    }

    // if we have a rollback, despawn any PreSpawned/DeterministicPredicted entities that were spawned since the rollback tick
    // (they will get respawned during the rollback)
    //
    // NOTE: if rollback happened at rollback_tick, then we will start running systems starting from rollback_tick + 1.
    //  so if the entity was spawned at tick >= rollback_tick + 1, we despawn it, and it can get respawned again
    if let Some(rollback_tick) = prediction_manager.get_rollback_start_tick() {
        debug!(
            ?rollback_tick,
            "Rollback! Despawning all PreSpawned/DeterministicPredicted entities spawned after the rollback tick"
        );
        // If the prespawned entity didn't exist at the rollback tick, despawn it
        prespawned_receiver.despawn_prespawned_after(rollback_tick + 1, &mut commands);

        // If the deterministic predicted entity didn't exist at the rollback tick, despawn it
        // We can drain everything because:
        // - entities spawned before the rollback_tick were created early enough to not need to be despawned
        //   and we don't want to check them again (since future rollbacks will happen even more in the future)
        // - entities spawned after the rollback tick will be despawned
        prediction_manager
            .deterministic_despawn
            .drain(..)
            .for_each(|(t, e)| {
                if t > rollback_tick
                    && let Ok(mut c) = commands.get_entity(e)
                {
                    c.despawn();
                }
            });

        // For skip_despawn, the tick is the first tick after which we should start enabling despawn on the entity
        // - if rollback_tick is bigger than the tick, then we remove DisableRollback and remove the entity from the vec because
        //   the entity was spawned a while ago and we want to enable rollbacks again
        // - for all remaining entities (where rollback_tick < tick) we insert DisableRollback
        let split_idx = prediction_manager
            .deterministic_skip_despawn
            .partition_point(|(t, _)| *t <= rollback_tick);
        let should_disable_rollback = prediction_manager
            .deterministic_skip_despawn
            .split_off(split_idx);
        should_disable_rollback.iter().for_each(|(_, e)| {
            if let Ok(mut c) = commands.get_entity(*e) {
                c.insert(DisableRollback);
            }
        });
        prediction_manager
            .deterministic_skip_despawn
            .iter()
            .for_each(|(_, e)| {
                if let Ok(mut c) = commands.get_entity(*e) {
                    c.remove::<DisableRollback>();
                }
            });
        // we only keep the entities for which we disabled rollback
        prediction_manager.deterministic_skip_despawn = should_disable_rollback;
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
    timeline: Res<LocalTimeline>,
    client: Single<AnyOf<(&LastConfirmedInput, &PredictionManager)>, With<IsSynced<InputTimeline>>>,
) {
    let (last_confirmed_input, prediction_manager) = client.into_inner();
    let tick = timeline.tick();

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

/// Before we start preparing for rollback, restore any PredictionDisable predicted entity
pub(crate) fn remove_prediction_disable(
    mut commands: Commands,
    query: Query<Entity, (With<Predicted>, With<PredictionDisable>)>,
) {
    query.iter().for_each(|e| {
        trace!(
            ?e,
            "Removing PredictionDisable marker before rollback preparation"
        );
        commands.entity(e).try_remove::<PredictionDisable>();
    });
}

/// If there is a mismatch, prepare rollback for all components.
///
/// Revert all components to the value in PredictionHistory<LastConfirmedTick>
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback<C: SyncComponent>(
    timeline: Res<LocalTimeline>,
    prediction_registry: Res<PredictionRegistry>,
    state_metadata: Res<StateRollbackMetadata>,
    mut commands: Commands,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (
            Entity,
            Option<&mut C>,
            Option<&ConfirmHistory>,
            &mut PredictionHistory<C>,
            AnyOf<(&Predicted, &PreSpawned, &DeterministicPredicted)>,
        ),
        Without<DisableRollback>,
    >,
    manager_query: Single<(&PredictionManager, &Rollback)>,
) {
    let kind = DebugName::type_name::<C>();
    let (manager, rollback) = manager_query.into_inner();
    let current_tick = timeline.tick();
    let _span = trace_span!("prepare_rollback", tick = ?current_tick, kind = ?kind).entered();
    let rollback_tick = manager.get_rollback_start_tick().unwrap();

    for (
        entity,
        predicted_component,
        confirm_history,
        mut predicted_history,
        (predicted, prespawned, disable_state_rollback),
    ) in predicted_query.iter_mut()
    {

        // if ConfirmTick < RollbackTick, then the earliest value in the history is confirmed
        //   (this is not true in all cases, but very probable)
        // and did not change. Set it as the value for the rollback tick
        if matches!(rollback, Rollback::FromState) {
            let confirm_tick: Tick = confirm_history.unwrap().last_tick().get().into();
            let last_confirmed_tick = state_metadata.last_confirmed_tick();
            if confirm_tick < last_confirmed_tick {
                let (_, oldest) = predicted_history.pop().unwrap();
                predicted_history.add(rollback_tick, oldest.into());
            }
        }

        // - if ConfirmTick >= RollbackTick, then we have more recent confirmed values written, but still the history
        //   contains values since LastConfirmedTick, so it should contain
        //   since LastConfirmedTick

        // 1. we need to clear the history so we can write a new one
        let correct_value = predicted_history.clear_except_tick(rollback_tick);

        if predicted.is_some() {
            // TODO: why aren't we always clearing the history? for prespawn?
            // clear all values as we will write a new history starting from rollback tick.
        predicted_history.clear();
        }

        let mut entity_mut = commands.entity(entity);
        // 3. we update the state to the Corrected state
        match correct_value {
            // confirm does not exist, remove on predicted
            None | Some(HistoryState::Removed) => {
                entity_mut.try_remove::<C>();
                trace!("Removing component from predicted entity for rollback");
            }
            // confirm exist, update or insert on predicted
            Some(HistoryState::Updated(correct)) => {
                match predicted_component {
                    None => {
                        debug!("Re-adding deleted component to predicted");
                        entity_mut.insert(correct);
                    }
                    Some(mut predicted_component) => {
                        // keep track of the current visual value so we can smooth the correction
                        if prediction_registry.has_correction::<C>() {
                            entity_mut.insert(PreviousVisual(predicted_component.clone()));
                            trace!(
                                ?entity,
                                previous_visual = ?predicted_component,
                                "Storing PreviousVisual for correction"
                            );
                        }

                        // update the component to the corrected value
                        *predicted_component = correct;
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
    let kind = DebugName::type_name::<R>();
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
    #[cfg(feature = "metrics")]
    let _timer = TimerGauge::new("prediction::rollback");

    let local_timeline = world.resource_mut::<LocalTimeline>();
    let current_tick = local_timeline.tick();
    let rollback_start_tick = world
        .query::<&PredictionManager>()
        .single(world)
        .unwrap()
        .get_rollback_start_tick()
        .expect("we should be in rollback");

    // NOTE: we reverted all components to the end of `current_roll
    let num_rollback_ticks = current_tick - rollback_start_tick;
    // reset the local timeline to be at the end of rollback_start_tick and we want to reach the end of current_tick
    world
        .resource_mut::<LocalTimeline>()
        .apply_delta(-num_rollback_ticks);
    debug!(
        "Rollback between {:?} and {:?}",
        rollback_start_tick, current_tick
    );
    #[cfg(feature = "metrics")]
    {
        metrics::counter!("prediction/rollback/count").increment(1);
        metrics::gauge!("prediction/rollback/ticks").set(num_rollback_ticks);
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
    metrics::gauge!("prediction/rollback/ticks").set(0);
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
