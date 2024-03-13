use std::fmt::Debug;

use bevy::app::FixedMain;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::{
    Commands, DespawnRecursiveExt, DetectChanges, Entity, Query, Ref, Res, ResMut, With, Without,
    World,
};
use tracing::{debug, error, trace, trace_span};

use crate::_reexport::{ComponentProtocol, FromType};
use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::prediction::correction::Correction;
use crate::client::prediction::predicted_history::ComponentState;
use crate::client::prediction::resource::PredictionManager;
use crate::prelude::client::SyncMetadata;
use crate::prelude::{PreSpawnedPlayerObject, TickManager};
use crate::protocol::Protocol;

use super::predicted_history::PredictionHistory;
use super::{Predicted, Rollback, RollbackState};

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_rollback<C: SyncComponent, P: Protocol>(
    // TODO: have a way to only get the updates of entities that are predicted?
    tick_manager: Res<TickManager>,
    connection: Res<ConnectionManager<P>>,

    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<&mut PredictionHistory<C>, (With<Predicted>, Without<Confirmed>)>,
    confirmed_query: Query<(Entity, Option<&C>, Ref<Confirmed>)>,
    mut rollback: ResMut<Rollback>,
) where
    <P as Protocol>::ComponentKinds: FromType<C>,
    P::Components: SyncMetadata<C>,
{
    let kind = <P::ComponentKinds as FromType<C>>::from_type();
    // TODO: maybe change this into a run condition so that we don't even run the system (reduces parallelism)
    if P::Components::mode() != ComponentSyncMode::Full {
        return;
    }

    // TODO: for mode=simple/once, we still need to re-add the component if the entity ends up not being despawned!
    if !connection.is_synced() || !connection.received_new_server_tick() {
        return;
    }

    let current_tick = tick_manager.tick();
    // TODO: can just enable bevy spans?
    let _span = trace_span!("client rollback check");

    for (confirmed_entity, confirmed_component, confirmed) in confirmed_query.iter() {
        // 0. only check rollback when any entity in the replication group has been updated
        // (i.e. the confirmed tick has been updated)
        if !confirmed.is_changed() {
            continue;
        }

        // let confirmed_entity = event.entity();
        // TODO: no need to get the Predicted component because we're not using it right now..
        //  we could use it in the future if we add more state in the Predicted Component
        // 1. Get the predicted entity, and it's history
        let Some(p) = confirmed.predicted else {
            continue;
        };
        let Ok(mut predicted_history) = predicted_query.get_mut(p) else {
            debug!("Predicted entity {:?} was not found", confirmed.predicted);
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

        // Note: it may seem like an optimization to only compare the history/server-state if we are not sure
        // that we should rollback (RollbackState::Default)
        // That is not the case, because if we do rollback we will need to snap the client entity to the server state
        // So either way we will need to do an operation.
        match rollback.state {
            // 3.a We are still not sure if we should do rollback. Compare history against confirmed
            // We rollback if there's no history (newly added predicted entity, or if there is a mismatch)
            RollbackState::Default => {
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
                        ComponentState::Updated(history_value) => history_value != *c,
                        ComponentState::Removed => true,
                    }),
                };
                if should_rollback {
                    debug!(
                   ?predicted_exist, ?confirmed_exist,
                   "Rollback check: mismatch for component between predicted and confirmed {:?} on tick {:?} for component {:?}. Current tick: {:?}",
                   confirmed_entity, tick, kind, current_tick
                   );
                    // TODO: try atomic enum update
                    rollback.state = RollbackState::ShouldRollback {
                        // we already rolled-back the state for the entity's latest_tick
                        // after this we will start right away with a physics update, so we need to start taking the inputs from the next tick
                        current_tick: tick + 1,
                    };
                }
            }
            // 3.b We already know we should do rollback (because of another entity/component), start the rollback
            RollbackState::ShouldRollback { .. } => {
                trace!(
                   "Rollback check: should roll back for component between predicted and confirmed on tick {:?} for component {:?}. Current tick: {:?}",
                   tick, kind, current_tick
                   );
            }
        };
    }
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback<C: SyncComponent, P: Protocol>(
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (
            Entity,
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
) where
    <P as Protocol>::ComponentKinds: FromType<C>,
    P::Components: SyncMetadata<C>,
{
    let kind = <P::ComponentKinds as FromType<C>>::from_type();

    // TODO: maybe change this into a run condition so that we don't even run the system (reduces parallelism)
    if P::Components::mode() != ComponentSyncMode::Full {
        return;
    }
    let _span = trace_span!("client rollback prepare");
    debug!("in prepare rollback");

    let current_tick = tick_manager.tick();
    for (confirmed_entity, confirmed_component, confirmed) in confirmed_query.iter() {
        let rollback_tick = confirmed.tick;
        //
        // // 0. Confirm that we are in rollback.
        // // NOTE: currently all predicted entities must be in the same replication group because I do not know how
        // //  to do a 'partial' rollback for only some entities
        // let Some(RollbackState::ShouldRollback { current_tick }) = rollback.state else {
        //     continue;
        // };
        // // careful, we added 1 to the tick in the check_rollback stage...
        // let tick = Tick(*current_tick - 1);

        let Some(p) = confirmed.predicted else {
            continue;
        };

        // 1. Get the predicted entity, and it's history
        let Ok((predicted_entity, predicted_component, mut predicted_history, mut correction)) =
            predicted_query.get_mut(p)
        else {
            debug!("Predicted entity {:?} was not found", confirmed.predicted);
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
                    .add_item(rollback_tick, ComponentState::Removed);
                entity_mut.remove::<C>();
            }
            // confirm exist, update or insert on predicted
            Some(c) => {
                predicted_history
                    .buffer
                    .add_item(rollback_tick, ComponentState::Updated(c.clone()));
                match predicted_component {
                    None => {
                        debug!("Re-adding deleted Full component to predicted");
                        entity_mut.insert(c.clone());
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
                        if correction_ticks != 0 && P::Components::has_correction() {
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
                                correction.current_correction = Some(c.clone());
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
                        *predicted_component = c.clone();
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
/// - TODO: entities that were despawned since rollback are respawned (maybe just via using prediction_despawn()?)
/// - TODO: do we need any correction?
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rollback_prespawn<C: SyncComponent, P: Protocol>(
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    mut prediction_manager: ResMut<PredictionManager>,
    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (
            Entity,
            Option<&mut C>,
            &mut PredictionHistory<C>,
            Option<&mut Correction<C>>,
        ),
        (
            With<PreSpawnedPlayerObject>,
            Without<Confirmed>,
            Without<Predicted>,
        ),
    >,
    rollback: Res<Rollback>,
) where
    <P as Protocol>::ComponentKinds: FromType<C>,
    P::Components: SyncMetadata<C>,
{
    let kind = <P::ComponentKinds as FromType<C>>::from_type();

    // TODO: maybe change this into a run condition so that we don't even run the system (reduces parallelism)
    // if P::Components::mode() != ComponentSyncMode::Full {
    //     return;
    // }
    let _span = trace_span!("client prepare rollback for pre-spawned entities");

    let current_tick = tick_manager.tick();

    let RollbackState::ShouldRollback {
        current_tick: rollback_tick_plus_one,
    } = rollback.state
    else {
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

    for (prespawned_entity, predicted_component, mut predicted_history, mut correction) in
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
                    // TODO: do we need to do a correction in this case?

                    // insert the Correction information only if the component exists on both confirmed and predicted
                    let correction_ticks = ((current_tick - rollback_tick) as f32
                        * config.prediction.correction_ticks_factor)
                        .round() as i16;

                    // no need to add the Correction if the correction is instant
                    if correction_ticks != 0 && P::Components::has_correction() {
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
                            correction.current_correction = Some(c.clone());
                        } else {
                            debug!("inserting new correction");
                            commands.entity(prespawned_entity).insert(Correction {
                                original_prediction: predicted_component.clone(),
                                original_tick: current_tick,
                                final_correction_tick,
                                current_visual: None,
                                current_correction: None,
                            });
                        }
                    }

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

pub(crate) fn run_rollback(world: &mut World) {
    let tick_manager = world.get_resource::<TickManager>().unwrap();
    let rollback = world.get_resource::<Rollback>().unwrap();
    let current_tick = tick_manager.tick();

    // NOTE: all predicted entities should be on the same tick!
    // TODO: might not need to check the state, because we only run this system if we are in rollback
    if let RollbackState::ShouldRollback {
        current_tick: current_rollback_tick,
    } = rollback.state
    {
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

        // run the physics fixed update schedule (which should contain ALL predicted/rollback components)
        for i in 0..num_rollback_ticks {
            // TODO: if we are in rollback, there are some FixedUpdate systems that we don't want to re-run ??
            //  for example we only want to run the physics on non-confirmed entities
            world.run_schedule(FixedMain)
        }
        debug!("Finished rollback. Current tick: {:?}", current_tick);
    }

    // revert the state of Rollback for the next frame
    let mut rollback = world.get_resource_mut::<Rollback>().unwrap();
    rollback.state = RollbackState::Default;
}

pub(crate) fn increment_rollback_tick(mut rollback: ResMut<Rollback>) {
    trace!("increment rollback tick");
    // update the rollback tick
    // (we already set the history for client.last_received_server_tick() in the rollback check,
    // we will start at the next tick. This is valid because this system runs after the physics systems)
    if let RollbackState::ShouldRollback {
        ref mut current_tick,
    } = rollback.state
    {
        *current_tick += 1;
    }
}

// #[cfg(test)]
// mod tests {
//     use bevy::utils::Duration;
//
//     use bevy::prelude::*;
//     use tracing::{debug, info};
//
//     use crate::_reexport::*;
//     use crate::prelude::client::*;
//     use crate::prelude::*;
//     use crate::tests::protocol::*;
//     use crate::tests::stepper::{BevyStepper, Step};
//
//     fn increment_component(
//         mut commands: Commands,
//         mut query: Query<(Entity, &mut Component1), With<Predicted>>,
//     ) {
//         for (entity, mut component) in query.iter_mut() {
//             component.0 += 1.0;
//             if component.0 == 5.0 {
//                 commands.entity(entity).remove::<Component1>();
//             }
//         }
//     }
//
//     fn setup() -> BevyStepper {
//         let frame_duration = Duration::from_millis(10);
//         let tick_duration = Duration::from_millis(10);
//         let shared_config = SharedConfig {
//             enable_replication: false,
//             tick: TickConfig::new(tick_duration),
//             log: LogConfig {
//                 level: tracing::Level::DEBUG,
//                 ..Default::default()
//             },
//             ..Default::default()
//         };
//         let link_conditioner = LinkConditionerConfig {
//             incoming_latency: Duration::from_millis(40),
//             incoming_jitter: Duration::from_millis(5),
//             incoming_loss: 0.05,
//         };
//         let sync_config = SyncConfig::default().speedup_factor(1.0);
//         let prediction_config = PredictionConfig::default().disable(false);
//         let interpolation_delay = Duration::from_millis(100);
//         let interpolation_config = InterpolationConfig::default().with_delay(InterpolationDelay {
//             min_delay: interpolation_delay,
//             send_interval_ratio: 0.0,
//         });
//         let mut stepper = BevyStepper::new(
//             shared_config,
//             sync_config,
//             prediction_config,
//             interpolation_config,
//             link_conditioner,
//             frame_duration,
//         );
//         stepper.client_mut().set_synced();
//         stepper.client_app.add_systems(
//             FixedUpdate,
//             increment_component
//         );
//         stepper
//     }
//
//     // Test that if a component gets removed from the predicted entity erroneously
//     // We are still able to rollback properly (the rollback adds the component to the predicted entity)
//     #[test]
//     fn test_removed_predicted_component_rollback() -> anyhow::Result<()> {
//         let mut stepper = setup();
//
//         // Create a confirmed entity
//         let confirmed = stepper
//             .client_app
//             .world
//             .spawn((Component1(0.0), ShouldBePredicted))
//             .id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world
//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Predicted>(predicted)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history got created
//         let mut history = PredictionHistory::<Component1>::default();
//         // this is added during the first rollback call after we create the history
//         history
//             .buffer
//             .add_item(Tick(0), ComponentState::Updated(Component1(0.0)));
//         history
//             .buffer
//             .add_item(Tick(1), ComponentState::Updated(Component1(1.0)));
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Component1>(predicted)
//                 .unwrap(),
//             &Component1(1.0)
//         );
//
//         // advance five more frames, so that the component gets removed on predicted
//         for i in 0..5 {
//             stepper.frame_step();
//         }
//         assert_eq!(stepper.client().tick(), Tick(6));
//
//         // check that the component got removed on predicted
//         assert!(stepper
//             .client_app
//             .world
//             .get::<Component1>(predicted)
//             .is_none());
//         // check that the component history is still there and that the value of the component history is correct
//         let mut history = PredictionHistory::<Component1>::default();
//         for i in 0..5 {
//             history
//                 .buffer
//                 .add_item(Tick(i), ComponentState::Updated(Component1(i as f32)));
//         }
//         history.buffer.add_item(Tick(5), ComponentState::Removed);
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//
//         // TODO: need to revisit this, a rollback situation is created from receiving a replication update now
//         // create a rollback situation
//         stepper.client_mut().set_synced();
//         stepper
//             .client_mut()
//             .set_latest_received_server_tick(Tick(3));
//         stepper
//             .client_app
//             .world
//             .get_mut::<Component1>(confirmed)
//             .unwrap()
//             .0 = 1.0;
//         // update without incrementing time, because we want to force a rollback check
//         stepper.client_app.update();
//
//         // check that rollback happened
//         // predicted got the component re-added
//         stepper
//             .client_app
//             .world
//             .get_mut::<Component1>(predicted)
//             .unwrap()
//             .0 = 4.0;
//         // check that the history is how we expect after rollback
//         let mut history = PredictionHistory::<Component1>::default();
//         for i in 3..7 {
//             history
//                 .buffer
//                 .add_item(Tick(i), ComponentState::Updated(Component1(i as f32 - 2.0)));
//         }
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history
//         );
//
//         Ok(())
//     }
//
//     // Test that if a component gets added to the predicted entity erroneously but didn't exist on the confirmed entity)
//     // We are still able to rollback properly (the rollback removes the component from the predicted entity)
//     #[test]
//     fn test_added_predicted_component_rollback() -> anyhow::Result<()> {
//         let mut stepper = setup();
//
//         // Create a confirmed entity
//         let confirmed = stepper.client_app.world.spawn(ShouldBePredicted).id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world
//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Predicted>(predicted)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // add a new component to Predicted
//         stepper
//             .client_app
//             .world
//             .entity_mut(predicted)
//             .insert(Component1(1.0));
//
//         // create a rollback situation (confirmed doesn't have a component that predicted has)
//         stepper.client_mut().set_synced();
//         stepper
//             .client_mut()
//             .set_latest_received_server_tick(Tick(1));
//         // update without incrementing time, because we want to force a rollback check
//         stepper.client_app.update();
//
//         // check that rollback happened: the component got removed from predicted
//         assert!(stepper
//             .client_app
//             .world
//             .get::<Component1>(predicted)
//             .is_none());
//
//         // check that history contains the removal
//         let mut history = PredictionHistory::<Component1>::default();
//         history.buffer.add_item(Tick(1), ComponentState::Removed);
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//         Ok(())
//     }
//
//     // Test that if a component gets removed from the confirmed entity
//     // We are still able to rollback properly (the rollback removes the component from the predicted entity)
//     #[test]
//     fn test_removed_confirmed_component_rollback() -> anyhow::Result<()> {
//         let mut stepper = setup();
//
//         // Create a confirmed entity
//         let confirmed = stepper
//             .client_app
//             .world
//             .spawn((Component1(0.0), ShouldBePredicted))
//             .id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world
//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Predicted>(predicted)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history got created
//         let mut history = PredictionHistory::<Component1>::default();
//         history
//             .buffer
//             .add_item(Tick(0), ComponentState::Updated(Component1(0.0)));
//         history
//             .buffer
//             .add_item(Tick(1), ComponentState::Updated(Component1(1.0)));
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history,
//         );
//
//         // create a rollback situation by removing the component on confirmed
//         stepper.client_mut().set_synced();
//         stepper
//             .client_mut()
//             .set_latest_received_server_tick(Tick(1));
//         stepper
//             .client_app
//             .world
//             .entity_mut(confirmed)
//             .remove::<Component1>();
//         // update without incrementing time, because we want to force a rollback check
//         // (need duration_since_latest_received_server_tick = 0)
//         stepper.client_app.update();
//
//         // check that rollback happened
//         // predicted got the component removed
//         assert!(stepper
//             .client_app
//             .world
//             .get_mut::<Component1>(predicted)
//             .is_none());
//
//         // check that the history is how we expect after rollback
//         let mut history = PredictionHistory::<Component1>::default();
//         history.buffer.add_item(Tick(1), ComponentState::Removed);
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history
//         );
//
//         Ok(())
//     }
//
//     // Test that if a component gets added to the confirmed entity (but didn't exist on the predicted entity)
//     // We are still able to rollback properly (the rollback adds the component to the predicted entity)
//     #[test]
//     fn test_added_confirmed_component_rollback() -> anyhow::Result<()> {
//         let mut stepper = setup();
//
//         // Create a confirmed entity
//         let confirmed = stepper.client_app.world.spawn(ShouldBePredicted).id();
//
//         // Tick once
//         stepper.frame_step();
//         assert_eq!(stepper.client().tick(), Tick(1));
//         let predicted = stepper
//             .client_app
//             .world
//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .predicted
//             .unwrap();
//
//         // check that the predicted entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<Predicted>(predicted)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history did not get created
//         assert!(stepper
//             .client_app
//             .world
//             .get::<PredictionHistory<Component1>>(predicted)
//             .is_none());
//
//         // advance five more frames, so that the component gets removed on predicted
//         for i in 0..5 {
//             stepper.frame_step();
//         }
//         assert_eq!(stepper.client().tick(), Tick(6));
//
//         // create a rollback situation by adding the component on confirmed
//         stepper.client_mut().set_synced();
//         stepper
//             .client_mut()
//             .set_latest_received_server_tick(Tick(3));
//         stepper
//             .client_app
//             .world
//             .entity_mut(confirmed)
//             .insert(Component1(1.0));
//         // update without incrementing time, because we want to force a rollback check
//         stepper.client_app.update();
//
//         // check that rollback happened
//         // predicted got the component re-added
//         stepper
//             .client_app
//             .world
//             .get_mut::<Component1>(predicted)
//             .unwrap()
//             .0 = 4.0;
//         // check that the history is how we expect after rollback
//         let mut history = PredictionHistory::<Component1>::default();
//         for i in 3..7 {
//             history
//                 .buffer
//                 .add_item(Tick(i), ComponentState::Updated(Component1(i as f32 - 2.0)));
//         }
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world
//                 .get::<PredictionHistory<Component1>>(predicted)
//                 .unwrap(),
//             &history
//         );
//
//         Ok(())
//     }
// }
