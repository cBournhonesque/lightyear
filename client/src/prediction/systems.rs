use crate::prediction::input_buffer::{InputBuffer, UserInput};
use crate::prediction::predicted_history::ComponentHistory;
use crate::prediction::{
    AtomicRollbackState, Confirmed, Predicted, PredictedComponent, Rollback, RollbackState,
};
use crate::Client;
use bevy::prelude::{Component, Entity, EventReader, FixedUpdate, Query, Res, ResMut, With, World};
use lightyear_shared::plugin::events::ComponentUpdateEvent;
use lightyear_shared::Protocol;
use std::ops::Deref;
use tracing::{error, info, trace_span, warn};

/// When we  want to create newly predicted entities, we need to:
/// - spawn an entity on the server for that client
/// - create a copy of that entity with Confirmed on the client
/// - replicate that copy to the server
// pub(crate) fn spawn_predicted(
//
// )

// TODO: it seems pretty suboptimal to have one system per component, refactor to loop through all components
//  ESPECIALLY BECAUSE WE ROLLBACK EVERYTHING IF ONE COMPONENT IS MISPREDICTED!
/// Systems that try to see if we should perform rollback for the predicted entity.
/// For each companent, we compare the confirmed component.
/// Should run every fixed-update
pub(crate) fn client_rollback_check<C: PredictedComponent, P: Protocol, T: UserInput>(
    // TODO: have a way to only get the updates of entities that are predicted?
    client: Res<Client<P>>,
    mut updates: EventReader<ComponentUpdateEvent<C>>,
    mut predicted_query: Query<(&Predicted, &mut ComponentHistory<C>)>,
    // confirmed contains the
    confirmed_query: Query<(&C, &Confirmed)>,
    mut input_buffer: ResMut<InputBuffer<T>>,
    mut rollback: ResMut<Rollback>,
) where
    <P as Protocol>::Components: From<C>,
{
    // TODO: can just enable bevy spans?
    let _span = trace_span!("client rollback check");

    // 0. We will compare the history and the confirmed entity for this tick
    // - Confirmed contains the server state at the tick
    // - History contains the history of what we predicted at the tick
    let latest_server_tick = client.latest_received_server_tick();

    // 1. Go through all server updates we received on this frame
    for event in updates.read() {
        // TODO: get the tick of the update from context if we switch to that
        let confirmed_entity = event.entity();
        if let Ok((confirmed_component, confirmed)) = confirmed_query.get(*confirmed_entity) {
            // TODO: no need to get the Predicted component because we're not using it right now..
            // 2. Get the predicted entity, and it's history
            if let Ok((predicted, mut predicted_history)) =
                predicted_query.get_mut(confirmed.predicted)
            {
                match rollback.state {
                    // 3. We are still not sure if we should do rollback. Compare history against confirmed
                    RollbackState::Default => {
                        if let Some(history_value) =
                            predicted_history.get_history_at_tick(latest_server_tick)
                        {
                            if history_value != *confirmed_component {
                                // we found a mismatch, we should rollback!
                                // TODO: try atomic enum update
                                rollback.state = RollbackState::ShouldRollback;
                            }
                        }
                    }
                    // We already know we should do rollback, stop
                    RollbackState::ShouldRollback => {
                        return;
                    }
                    _ => {
                        error!("Rollback state should not be in rollback here")
                    }
                }
            } else {
                warn!("Predicted entity {:?} was not found", confirmed.predicted);
            }
        } else {
            warn!(
                "Confirmed entity from UpdateEvent {:?} was not found",
                confirmed_entity
            );
        }
    }
}

pub(crate) fn run_rollback<P: Protocol>(world: &mut World, mut rollback: ResMut<Rollback>) {
    let client = world.get_resource::<Client<P>>().unwrap();
    let num_rollback_ticks = client.tick() - client.latest_received_server_tick() + 1;
    info!(
        "Rollback between {:?} and {:?}",
        client.latest_received_server_tick(),
        client.tick()
    );
    match rollback.state {
        RollbackState::ShouldRollback => {
            // TODO: CLEAR THE EXISTING CLIENT HISTORY SINCE THE ROLLBACK TICKS! (can just clear the entire history basically)

            // run the physics fixed update schedule (which should contain ALL predicted/rollback components)
            for i in 0..num_rollback_ticks {
                // TODO: if we are in rollback, there are some FixedUpdate systems that we don't want to re-run ??
                world.run_schedule(FixedUpdate)
            }
            rollback.state = RollbackState::DidRollback;
        }
        _ => rollback.state = RollbackState::Default,
    }
}
