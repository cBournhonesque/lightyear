use bevy::prelude::*;
use core::time::Duration;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour};
// Use server ActionState
use lightyear::prelude::server::{ActionState, ClientOf, Replicate, ReplicationSender, ServerPlugin};
use lightyear::prelude::*;
use lightyear_examples_common_new::shared::SEND_INTERVAL; // Import SEND_INTERVAL
// Removed InitialReplicated and client components
// use lightyear::client::components::Confirmed;
// use lightyear::client::interpolation::Interpolated;
// use lightyear::client::prediction::Predicted;
// use lightyear::inputs::native::ActionState;
// use lightyear::shared::replication::components::InitialReplicated;


// Plugin for server-specific logic
#[derive(Clone)] // Added Clone
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        // Removed init system
        // app.add_systems(Startup, init);
        // Use observers instead of PreUpdate systems
        app.add_observer(replicate_cursors);
        app.add_observer(replicate_players);
        // app.add_systems(
        //     PreUpdate,
        //     (replicate_cursors, replicate_players).in_set(ServerReplicationSet::ClientReplication),
        // );
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, (movement, delete_player));
    }
}

// Removed init system
// pub(crate) fn init(mut commands: Commands) {
//     commands.start_server();
// }

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<
        (&mut PlayerPosition, &ActionState<Inputs>),
        // if we run in host-server mode, we don't want to apply this system to the local client's entities
        // because they are already moved by the client plugin
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    for (position, inputs) in position_query.iter_mut() {
        if let Some(input) = &inputs.value {
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared_movement_behaviour(position, input);
        }
    }
}

fn delete_player(
    mut commands: Commands,
    query: Query<(Entity, &ActionState<Inputs>), With<PlayerPosition>>,
) {
    for (entity, inputs) in query.iter() {
        if inputs.value.as_ref().is_some_and(|v| v == &Inputs::Delete) {
            // You can try 2 things here:
            // - either you consider that the client's action is correct, and you despawn the entity. This should get replicated
            //   to other clients.
            // - you decide that the client's despawn is incorrect, and you do not despawn the entity. Then the client's prediction
            //   should be rolled back, and the entity should not get despawned on client.
            commands.entity(entity).despawn();
        }
    }
}

// Replicate the pre-predicted entities back to the client.
//
// The objective was to create a normal 'predicted' entity directly in the client timeline, instead
// of having to create the entity in the server timeline and wait for it to be replicated.
// Note that this needs to run before FixedUpdate, since we handle client inputs in the FixedUpdate schedule (subject to change)
// And we want to handle deletion properly
// Changed trigger and query to use ClientOf
pub(crate) fn replicate_players(
    trigger: Trigger<OnAdd, ClientOf>,
    mut commands: Commands,
    player_query: Query<&PlayerPosition>, // Check if it's a player entity
    client_query: Query<&ClientOf>, // Query ClientOf to get PeerId
) {
    let entity = trigger.target();
    // Check if the entity that connected has a PlayerPosition component
    if player_query.get(entity).is_err() {
        return;
    }

    let Ok(client_of) = client_query.get(entity) else {
        error!("ClientOf component not found for entity {entity:?}");
        return;
    };
    let client_id = client_of.peer_id; // Use PeerId
    debug!("received player spawn event from {client_id:?}");

    if let Some(mut e) = commands.get_entity(entity) {
        // Standard prediction: predict owner, interpolate others
        let prediction_target = PredictionTarget::to_clients(NetworkTarget::Single(client_id));
        let interpolation_target = InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id));

        e.insert((
            // Replicate to all clients, including the owner
            Replicate::to_clients(NetworkTarget::All),
            prediction_target,
            interpolation_target,
            // Add ReplicationSender to send updates back to clients
            ReplicationSender::new(
                SEND_INTERVAL,
                SendUpdatesMode::SinceLastAck,
                false,
            ),
            // Removed OverrideTargetComponent::<PrePredicted>
        ));
    }
}

// Changed trigger and query to use ClientOf
pub(crate) fn replicate_cursors(
    trigger: Trigger<OnAdd, ClientOf>,
    mut commands: Commands,
    cursor_query: Query<&CursorPosition>, // Check if it's a cursor entity
    client_query: Query<&ClientOf>, // Query ClientOf to get PeerId
) {
    let entity = trigger.target();
    // Check if the entity that connected has a CursorPosition component
    if cursor_query.get(entity).is_err() {
        return;
    }

    let Ok(client_of) = client_query.get(entity) else {
        error!("ClientOf component not found for entity {entity:?}");
        return;
    };
    let client_id = client_of.peer_id; // Use PeerId
    info!("received cursor spawn event from client: {client_id:?}");

    if let Some(mut e) = commands.get_entity(entity) {
        // Cursor: replicate to others, interpolate for others
        let interpolation_target = InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id));

        e.insert((
            // Replicate only to other clients (server doesn't need owner's cursor, owner handles it locally)
            Replicate::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            // No prediction for cursor
            PredictionTarget::to_clients(NetworkTarget::None),
            interpolation_target,
            // Add ReplicationSender to send updates back to clients
            ReplicationSender::new(
                SEND_INTERVAL,
                SendUpdatesMode::SinceLastAck,
                false,
            ),
        ));
    }
}
