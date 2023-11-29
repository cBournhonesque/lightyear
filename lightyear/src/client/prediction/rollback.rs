use std::fmt::Debug;

use bevy::prelude::{Commands, Entity, FixedUpdate, Query, Res, ResMut, Without, World};
use tracing::{debug, error, info, trace, trace_span, warn};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::prediction::predicted_history::ComponentState;
use crate::client::resource::Client;
use crate::protocol::Protocol;

use super::predicted_history::PredictionHistory;
use super::{Predicted, Rollback, RollbackState};

/// When we  want to create newly predicted entities, we need to:
/// - spawn an entity on the server for that client
/// - create a copy of that entity with Confirmed on the client
/// - replicate that copy to the server
// pub(crate) fn spawn_predicted(
//
// )

// TODO: maybe add a condition for running rollback
// pub(crate) fn should_rollback_check(rollback: Res<Rollback>) {
//
// }

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
//
//
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
/// For each companent, we compare the confirmed component (server-state) with the history.
/// If we need to run rollback, we clear the predicted history and snap the history back to the server-state
// TODO: do not rollback if client is not time synced
#[allow(clippy::type_complexity)]
pub(crate) fn client_rollback_check<C: SyncComponent, P: Protocol>(
    // TODO: have a way to only get the updates of entities that are predicted?
    mut commands: Commands,
    client: Res<Client<P>>,

    // mut updates: EventReader<ComponentUpdateEvent<C>>,
    // mut inserts: EventReader<ComponentInsertEvent<C>>,
    // mut removals: EventReader<ComponentRemoveEvent<C>>,

    // TODO: here we basically snap back the value only for the ComponentHistory components
    //  but we might want to copy the predictive state of some components from Confirmed to Predicted, without caring
    //  about rollback
    //  So there would be three states:
    //  #[replication(predicted)]  -> do full rollback
    //  #[replication(copy)]  -> prediction just copy the state of the server when it arrives (even if the server state is a bit late. Useful for
    //     components that don't get updated often, such as Color, Name, etc.)
    //  -> no replication at all, the component is replicated to the Confirmed entity but not the predicted one.
    //  IDEALLY THIS GET BE SET PER ENTITY, NOT IN THE PROTOCOL ITSELF?
    //  MAYBE A PREDICTION-STATE for each component, similar to prediction history?

    // We also snap the value of the component to the server state if we are in rollback
    // We use Option<> because the predicted component could have been removed while it still exists in Confirmed
    mut predicted_query: Query<
        (
            Entity,
            &Predicted,
            Option<&mut C>,
            &mut PredictionHistory<C>,
        ),
        Without<Confirmed>,
    >,
    confirmed_query: Query<(Entity, Option<&C>, &Confirmed)>,
    mut rollback: ResMut<Rollback>,
)
// where
// <P as Protocol>::Components: From<C>,
{
    // TODO: maybe change this into a run condition so that we don't even run the system (reduces parallelism)
    if C::mode() != ComponentSyncMode::Full {
        return;
    }
    if !client.is_synced() || !client.received_new_server_tick() {
        trace!(
            sync = ?client.is_synced(),
            received_new_server_tick = ?client.received_new_server_tick(),
            duration_since_last_server_tick = ?client.duration_since_latest_received_server_tick(),
            "Not running rollback check because client is not synced or didn't receive new server tick");
        return;
    }
    // TODO: can just enable bevy spans?
    let _span = trace_span!("client rollback check");

    // 0. We will compare the history and the confirmed entity for this tick
    // - Confirmed contains the server state at the tick
    // - History contains the history of what we predicted at the tick
    let latest_server_tick = client.latest_received_server_tick();

    // 1. We want to do a rollback check every time the component got modified (removed/added/updated) on the confirmed entity
    // NOTE: actually we should just check for rollback if latest_received_server_tick got modified
    //  because even if server state didn't change, our history did

    // let confirmed_entity_updates = updates
    //     .read()
    //     .map(|event| event.entity())
    //     .chain(inserts.read().map(|event| event.entity()))
    //     .chain(removals.read().map(|event| event.entity()));
    // TODO: does this contain duplicates? if so, we should dedup

    // TODO: should we clear these? so that we don't see them again in the next tick?
    //  or should we just keep track of the last time we ran this system, and what the last_received_server_tick was.
    //  if the last_received_server_tick didn't change, then no point in redoing the rollback check

    // for event in updates.read() {
    for (confirmed_entity, confirmed_component, confirmed) in confirmed_query.iter() {
        // TODO: get the tick of the update from context of ComponentUpdateEvent if we switch to that
        // let confirmed_entity = event.entity();
        // TODO: no need to get the Predicted component because we're not using it right now..
        //  we could use it in the future if we add more state in the Predicted Component
        // 2. Get the predicted entity, and it's history
        if let Some(p) = confirmed.predicted {
            if let Ok((predicted_entity, predicted, predicted_component, mut predicted_history)) =
                predicted_query.get_mut(p)
            {
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
                        let history_value = predicted_history.pop_until_tick(latest_server_tick);
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
                                "Rollback check: mismatch for component between predicted and confirmed {:?}",
                                confirmed_entity
                            );
                            // info!(
                            //     "Rollback check: mismatch for component {:?} between predicted and confirmed {:?}", C::name(),
                            //     confirmed_entity
                            // );
                            // TODO (unrelated): pattern for enabling replication-behaviour for a component/entity.
                            //  Added a ReplicationBehaviour<C>.
                            //  And then maybe we can add an EntityCommands extension that adds a ReplicationBehaviour<C>

                            // we need to clear the history so we can write a new one
                            predicted_history.clear();
                            // SAFETY: we know the predicted entity exists
                            let mut entity_mut = commands.entity(predicted_entity);
                            match confirmed_component {
                                // confirm does not exist, remove on predicted
                                None => {
                                    predicted_history
                                        .buffer
                                        .add_item(latest_server_tick, ComponentState::Removed);
                                    entity_mut.remove::<C>();
                                }
                                // confirm exist, update or insert on predicted
                                Some(c) => {
                                    predicted_history.buffer.add_item(
                                        latest_server_tick,
                                        ComponentState::Updated(c.clone()),
                                    );
                                    match predicted_component {
                                        None => {
                                            entity_mut.insert(c.clone());
                                        }
                                        Some(mut predicted_component) => {
                                            *predicted_component = c.clone();
                                        }
                                    };
                                }
                            };
                            // TODO: try atomic enum update
                            rollback.state = RollbackState::ShouldRollback {
                                // we already replicated the latest_server_tick state
                                // after this we will start right away with a physics update, so we need to start taking the inputs from the next tick
                                current_tick: latest_server_tick + 1,
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
                                    .add_item(latest_server_tick, ComponentState::Removed);
                                entity_mut.remove::<C>();
                            }
                            // confirm exist, update or insert on predicted
                            Some(c) => {
                                predicted_history.buffer.add_item(
                                    latest_server_tick,
                                    ComponentState::Updated(c.clone()),
                                );
                                match predicted_component {
                                    None => {
                                        entity_mut.insert(c.clone());
                                    }
                                    Some(mut predicted_component) => {
                                        *predicted_component = c.clone();
                                    }
                                };
                            }
                        };
                        // return;
                    }
                }
            } else {
                warn!("Predicted entity {:?} was not found", confirmed.predicted);
            }
        }
    }
}

// TODO: check how we handle the user inputs here??
pub(crate) fn run_rollback<P: Protocol>(world: &mut World) {
    let client = world.get_resource::<Client<P>>().unwrap();
    let num_rollback_ticks = client.tick() - client.latest_received_server_tick();
    debug!(
        "Rollback between {:?} and {:?}",
        client.latest_received_server_tick(),
        client.tick()
    );

    let rollback = world.get_resource::<Rollback>().unwrap();
    // TODO: might not need to check the state, because we only run this system if we are in rollback
    if let RollbackState::ShouldRollback { .. } = rollback.state {
        // run the physics fixed update schedule (which should contain ALL predicted/rollback components)
        for i in 0..num_rollback_ticks {
            // TODO: if we are in rollback, there are some FixedUpdate systems that we don't want to re-run ??
            //  for example we only want to run the physics on non-confirmed entities
            world.run_schedule(FixedUpdate)
        }
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
