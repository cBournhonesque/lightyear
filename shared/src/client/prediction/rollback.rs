use bevy::prelude::{EventReader, FixedUpdate, Query, Res, ResMut, Without, World};
use tracing::{error, info, trace_span, warn};

use crate::client::Client;
use crate::plugin::events::{ComponentInsertEvent, ComponentUpdateEvent};
use crate::Protocol;

use super::predicted_history::ComponentHistory;
use super::{Confirmed, Predicted, PredictedComponent, Rollback, RollbackState};

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

// TODO: it seems pretty suboptimal to have one system per component, refactor to loop through all components
//  ESPECIALLY BECAUSE WE ROLLBACK EVERYTHING IF ONE COMPONENT IS MISPREDICTED!
/// Systems that try to see if we should perform rollback for the predicted entity.
/// For each companent, we compare the confirmed component (server-state) with the history.
/// If we need to run rollback, we clear the predicted history and snap the history back to the server-state
// TODO: do not rollback if client is not time synced
pub(crate) fn client_rollback_check<C: PredictedComponent, P: Protocol>(
    // TODO: have a way to only get the updates of entities that are predicted?
    client: Res<Client<P>>,
    mut updates: EventReader<ComponentUpdateEvent<C>>,
    mut inserts: EventReader<ComponentInsertEvent<C>>,
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
    mut predicted_query: Query<(&Predicted, &mut C, &mut ComponentHistory<C>), Without<Confirmed>>,
    confirmed_query: Query<(&C, &Confirmed)>,
    mut rollback: ResMut<Rollback>,
)
// where
// <P as Protocol>::Components: From<C>,
{
    if !client.is_synced() {
        return;
    }
    // TODO: can just enable bevy spans?
    let _span = trace_span!("client rollback check");

    // 0. We will compare the history and the confirmed entity for this tick
    // - Confirmed contains the server state at the tick
    // - History contains the history of what we predicted at the tick
    let latest_server_tick = client.latest_received_server_tick();

    // 1. We want to do a rollback check every time a component got updated/inserted on this frame
    let updated_confirmed_entities = updates
        .read()
        .map(|event| event.entity())
        .chain(inserts.read().map(|event| event.entity()));
    // for event in updates.read() {
    for confirmed_entity in updated_confirmed_entities {
        // TODO: get the tick of the update from context of ComponentUpdateEvent if we switch to that
        // let confirmed_entity = event.entity();
        if let Ok((confirmed_component, confirmed)) = confirmed_query.get(*confirmed_entity) {
            // TODO: no need to get the Predicted component because we're not using it right now..
            //  we could use it in the future if we add more state in the Predicted Component
            // 2. Get the predicted entity, and it's history
            if let Ok((predicted, mut predicted_component, mut predicted_history)) =
                predicted_query.get_mut(confirmed.predicted)
            {
                // Note: it may seem like an optimization to only compare the history/server-state if we are not sure
                // that we should rollback (RollbackState::Default)
                // That is not the case, because if we do rollback we will need to snap the client entity to the server state
                // So either way we will need to do an operation.
                match rollback.state {
                    // 3.a We are still not sure if we should do rollback. Compare history against confirmed
                    // We rollback if there's no history (newly added predicted entity, or if there is a mismatch)
                    RollbackState::Default => {
                        let history_value = predicted_history.pop_until_tick(latest_server_tick);
                        let should_rollback = history_value
                            .map_or(true, |history_value| history_value != *confirmed_component);
                        if should_rollback {
                            // info!(
                            //     "Rollback check: mismatch for component {:?} between predicted and confirmed {:?}", C::name(),
                            //     confirmed_entity
                            // );
                            // TODO (unrelated): pattern for enabling replication-behaviour for a component/entity.
                            //  Added a ReplicationBehaviour<C>.
                            //  And then maybe we can add an EntityCommands extension that adds a ReplicationBehaviour<C>

                            // TODO: WE ACTUALLY DONT NEED TO CLEAR THE HISTORY BECAUSE WE WILL ROLLBACK ANYWAY.
                            predicted_history.clear();
                            // TODO: WE DON'T NEED TO WRITE THE HISTORY HERE, BECAUSE WE WILL NEVER USE THIS TICK AGAIN NORMALLY!
                            predicted_history
                                .buffer
                                .add_item(latest_server_tick, confirmed_component.clone());
                            *predicted_component = confirmed_component.clone();
                            // TODO: try atomic enum update
                            rollback.state = RollbackState::ShouldRollback {
                                // we already replicated the latest_server_tick state
                                // after this we will start right away with a physics update, so we need to start taking the inputs from the next tick
                                current_tick: latest_server_tick + 1,
                            };
                        }
                    }
                    // 3.b We already know we should do rollback, clear the history and snap the predicted history to the server state
                    RollbackState::ShouldRollback { .. } => {
                        predicted_history.clear();
                        predicted_history
                            .buffer
                            .add_item(latest_server_tick, confirmed_component.clone());
                        *predicted_component = confirmed_component.clone();
                        return;
                    }
                    _ => {
                        error!("Rollback state should not be in rollback here")
                    }
                }
            } else {
                // warn!("Predicted entity {:?} was not found", confirmed.predicted);
            }
        } else {
            /*warn!(
                "Confirmed entity from UpdateEvent {:?} was not found",
                confirmed_entity
            );*/
        }
    }
}

// TODO: check how we handle the user inputs here??
pub(crate) fn run_rollback<P: Protocol>(world: &mut World) {
    let client = world.get_resource::<Client<P>>().unwrap();
    let num_rollback_ticks = client.tick() - client.latest_received_server_tick();
    info!(
        "Rollback between {:?} and {:?}",
        client.latest_received_server_tick(),
        client.tick()
    );

    let rollback = world.get_resource::<Rollback>().unwrap();
    // TODO: might not need to check the state, because we only run this system if we are in rollback
    match rollback.state {
        RollbackState::ShouldRollback { .. } => {
            // run the physics fixed update schedule (which should contain ALL predicted/rollback components)
            for i in 0..num_rollback_ticks {
                // TODO: if we are in rollback, there are some FixedUpdate systems that we don't want to re-run ??
                //  for example we only want to run the physics on non-confirmed entities
                world.run_schedule(FixedUpdate)
            }
        }
        _ => {}
    }

    // revert the state of Rollback for the next frame
    let mut rollback = world.get_resource_mut::<Rollback>().unwrap();
    rollback.state = RollbackState::Default;
}

pub(crate) fn increment_rollback_tick(mut rollback: ResMut<Rollback>) {
    info!("increment rollback tick");
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
