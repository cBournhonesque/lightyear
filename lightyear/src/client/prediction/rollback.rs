use std::fmt::Debug;

use crate::_reexport::FromType;
use bevy::prelude::{
    Commands, DetectChanges, Entity, EventReader, FixedUpdate, Query, Ref, Res, ResMut, With,
    Without, World,
};
use bevy::utils::EntityHashSet;
use tracing::{debug, error, info, trace, trace_span};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::events::{ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent};
use crate::client::prediction::correction::Correction;
use crate::client::prediction::predicted_history::ComponentState;
use crate::client::resource::Client;
use crate::prelude::client::SyncMetadata;
use crate::protocol::Protocol;
use crate::shared::tick_manager::TickManaged;

use super::predicted_history::PredictionHistory;
use super::{Predicted, Rollback, RollbackState};

// TODO (unrelated): pattern for enabling replication-behaviour for a component/entity. (for example don't replicate this component)
//  Added a ReplicationBehaviour<C>.
//  And then maybe we can add an EntityCommands extension that adds a ReplicationBehaviour<C>

// NOTE: for rollback to work, all entities that are predicted need to be replicated on the same tick!

// We need:
// - Removed because we need to know if the component was removed on predicted, but we have to keep the history for the rest of rollback
// - To be able to handle missing component histories; (if a component suddenly gets added on confirmed for the first time, it won't exist on predicted, so existed wont have a history)
//   - BUT WE COULD JUST SPAWN A HISTORY FOR PREDICTED AS SOON AS WE RECEIVE THAT EVENT?
// - Add ComponentHistory if a component gets added on predicted; so we can start accumulating history for future rollbacks
// - Add ComponentHistory if a component gets added on confirmed; so we can initiate rollback (if predicted didn't have this component)
// - We don't really need to remove ComponentHistory. We could try to do it as an optimization later on. For now we just keep them around.

// TODO: it seems pretty suboptimal to have one system per component, refactor to loop through all components
//  ESPECIALLY BECAUSE WE ROLLBACK EVERYTHING IF ONE COMPONENT IS MISPREDICTED!
/// Systems that try to see if we should perform rollback for the predicted entity.
/// For each component, we compare the confirmed component (server-state) with the history.
/// If we need to run rollback, we clear the predicted history and snap the history back to the server-state
// TODO: do not rollback if client is not time synced
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn client_rollback_check<C: SyncComponent, P: Protocol>(
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    client: Res<Client<P>>,

    // mut updates: EventReader<ComponentUpdateEvent<C>>,
    // mut inserts: EventReader<ComponentInsertEvent<C>>,
    // mut removals: EventReader<ComponentRemoveEvent<C>>,

    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (Entity, Option<&mut C>, &mut PredictionHistory<C>),
        (With<Predicted>, Without<Confirmed>),
    >,
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
    if !client.is_synced() || !client.received_new_server_tick() {
        return;
    }
    // TODO: can just enable bevy spans?
    let _span = trace_span!("client rollback check");

    // // 0. We want to do a rollback check every time the component for the confirmed entity got modified in any way (removed/added/updated)
    // let confirmed_entity_with_updates = updates
    //     .read()
    //     .map(|event| event.entity())
    //     .chain(inserts.read().map(|event| event.entity()))
    //     .chain(removals.read().map(|event| event.entity()))
    //     .collect::<EntityHashSet<Entity>>();

    for (confirmed_entity, confirmed_component, confirmed) in confirmed_query.iter() {
        // only check rollback when any entity in the replication group has been updated
        if !confirmed.is_changed() {
            continue;
        }
        // for confirmed_entity in confirmed_entity_with_updates {
        //     let Ok((confirmed_component, confirmed)) = confirmed_query.get(confirmed_entity) else {
        //         // this could happen if the entity was despawned but we received updates for it.
        //         // maybe only send events for an entity if it still exists?
        //         debug!(
        //             "could not find the confirmed entity: {:?} that received an update",
        //             confirmed_entity
        //         );
        //         continue;
        //     };

        // TODO: get the tick of the update from context of ComponentUpdateEvent if we switch to that
        // let confirmed_entity = event.entity();
        // TODO: no need to get the Predicted component because we're not using it right now..
        //  we could use it in the future if we add more state in the Predicted Component
        // 1. Get the predicted entity, and it's history
        if let Some(p) = confirmed.predicted {
            let Ok((predicted_entity, predicted_component, mut predicted_history)) =
                predicted_query.get_mut(p)
            else {
                debug!("Predicted entity {:?} was not found", confirmed.predicted);
                continue;
            };

            // 2. We will compare the predicted history and the confirmed entity at the current confirmed entity tick
            // - Confirmed contains the server state at the tick
            // - History contains the history of what we predicted at the tick
            // get the tick that the confirmed entity is at
            let tick = confirmed.tick;
            if tick > client.tick() {
                info!(
                    "Confirmed entity {:?} is at a tick in the future: {:?} compared to client timeline. Current tick: {:?}",
                    confirmed_entity,
                    tick,
                    client.tick()
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
                    // rollback table:
                    // - confirm exist. rollback if:
                    //    - predicted history exists and is different
                    //    - predicted history does not exist
                    //    To rollback:
                    //    - update the predicted component to the confirmed component if it exists
                    //    - insert the confirmed component to the predicted entity if it doesn't exist
                    // - confirm does not exist. rollback if:
                    //    - predicted history exists and doesn't contain Removed
                    //    -
                    //    To rollback:
                    //    - we remove the component from predicted.

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
                        Some(c) => {
                            history_value.map_or(true, |history_value| match history_value {
                                ComponentState::Updated(history_value) => history_value != *c,
                                ComponentState::Removed => true,
                            })
                        }
                    };
                    if should_rollback {
                        info!(
                            ?predicted_exist, ?confirmed_exist,
                                "Rollback check: mismatch for component between predicted and confirmed {:?} on tick {:?} for component {:?}. Current tick: {:?}",
                                confirmed_entity, tick, kind, client.tick()
                        );

                        // we need to clear the history so we can write a new one
                        predicted_history.clear();
                        // SAFETY: we know the predicted entity exists
                        let mut entity_mut = commands.entity(predicted_entity);

                        // we update the state to the Corrected state
                        // NOTE: visually, we will use the CorrectionFn to interpolate between the current Predicted state and the Corrected state
                        //  even though for other purposes (physics, etc.) we switch directly to the Corrected state
                        match confirmed_component {
                            // confirm does not exist, remove on predicted
                            None => {
                                predicted_history
                                    .buffer
                                    .add_item(tick, ComponentState::Removed);
                                entity_mut.remove::<C>();
                            }
                            // confirm exist, update or insert on predicted
                            Some(c) => {
                                predicted_history
                                    .buffer
                                    .add_item(tick, ComponentState::Updated(c.clone()));
                                match predicted_component {
                                    None => {
                                        debug!("Re-adding deleted Full component to predicted");
                                        entity_mut.insert(c.clone());
                                    }
                                    Some(mut predicted_component) => {
                                        // insert the Correction information only if the component exists on both confirmed and predicted
                                        let correction_ticks =
                                            client.config().prediction.correction_ticks;
                                        // no need to add the Correction if the correction is instant
                                        if correction_ticks != 0 {
                                            entity_mut.insert(Correction {
                                                original_prediction: predicted_component.clone(),
                                                original_tick: client.tick(),
                                                final_correction_tick: client.tick()
                                                    + correction_ticks as i16,
                                                current_correction: None,
                                            });
                                        }
                                        *predicted_component = c.clone();
                                    }
                                };
                            }
                        };
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
                    // we need to clear the history so we can write a new one
                    predicted_history.clear();

                    // SAFETY: we know the predicted entity exists
                    let mut entity_mut = commands.entity(predicted_entity);
                    match confirmed_component {
                        // confirm does not exist, remove on predicted
                        None => {
                            predicted_history
                                .buffer
                                .add_item(tick, ComponentState::Removed);
                            entity_mut.remove::<C>();
                        }
                        // confirm exist, update or insert on predicted
                        Some(c) => {
                            predicted_history
                                .buffer
                                .add_item(tick, ComponentState::Updated(c.clone()));
                            match predicted_component {
                                None => {
                                    debug!("Re-adding deleted Full component to predicted");
                                    entity_mut.insert(c.clone());
                                }
                                Some(mut predicted_component) => {
                                    // insert the Correction information only if the component exists on both confirmed and predicted
                                    let correction_ticks =
                                        client.config().prediction.correction_ticks;
                                    // no need to add the Correction if the correction is instant
                                    if correction_ticks != 0 {
                                        entity_mut.insert(Correction {
                                            original_prediction: predicted_component.clone(),
                                            original_tick: client.tick(),
                                            final_correction_tick: client.tick()
                                                + correction_ticks as i16,
                                            current_correction: None,
                                        });
                                    }
                                    *predicted_component = c.clone();
                                }
                            };
                        }
                    };
                }
            }
        }
    }
}

pub(crate) fn run_rollback<P: Protocol>(world: &mut World) {
    let client = world.get_resource::<Client<P>>().unwrap();
    let rollback = world.get_resource::<Rollback>().unwrap();

    let current_tick = client.tick();

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
            world.run_schedule(FixedUpdate)
        }
        info!("Finished rollback. Current tick: {:?}", current_tick);
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
//
// #[cfg(test)]
// mod tests {
//     use std::time::Duration;
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
//             increment_component.in_set(FixedUpdateSet::Main),
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
