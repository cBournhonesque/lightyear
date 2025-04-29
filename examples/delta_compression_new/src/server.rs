//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use crate::protocol::*;
use crate::shared;
use bevy::app::PluginGroupBuilder;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
// Use server ActionState, new connection types, replication components
use lightyear::prelude::server::{ActionState, ClientOf, Connected, Replicate, ReplicationSender, ServerConnectionManager, ServerPlugin};
use lightyear::prelude::*;
use std::sync::Arc;
use lightyear_examples_common_new::shared::SEND_INTERVAL; // Import SEND_INTERVAL

#[derive(Clone)] // Added Clone
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientEntityMap>();
        // Removed start_server system
        // app.add_systems(Startup, start_server);
        // the physics/FixedUpdates systems that consume inputs should be run in this set.
        app.add_systems(FixedUpdate, movement);
        // Use observers for connections/disconnections
        app.add_observer(handle_connected);
        app.add_observer(handle_disconnected);
        app.add_systems(Update, send_message);
        // app.add_systems(Update, (send_message, handle_connections)); // Removed old handler
    }
}

/// A simple resource map that tell me  the corresponding server entity of that client
/// Important for O(n) acess
#[derive(Resource, Default)]
pub struct ClientEntityMap(HashMap<PeerId, Entity>); // Use PeerId

// Removed start_server system
// fn start_server(mut commands: Commands) {
//     commands.start_server();
// }

/// Spawn player entity when a client connects
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>, // Trigger on connection
    mut entity_map: ResMut<ClientEntityMap>,
    mut commands: Commands,
    query: Query<&Connected>, // Query Connected component
) {
    let client_entity = trigger.target();
    let Ok(connected) = query.get(client_entity) else { return };
    let client_id = connected.peer_id; // Get PeerId

    // Standard prediction: predict owner, interpolate others
    let prediction_target = PredictionTarget::to_clients(NetworkTarget::Single(client_id));
    let interpolation_target = InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id));

    let replicate = Replicate::to_clients(NetworkTarget::All); // Replicate to all

    let entity_commands = commands.spawn((
        PlayerBundle::new(client_id, Vec2::ZERO),
        replicate,
        prediction_target,
        interpolation_target,
        // Add ReplicationSender to send updates back to clients
        ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ),
    ));
    let entity = entity_commands.id();
    entity_map.0.insert(client_id, entity);

    info!("Create entity {:?} for client {:?}", entity, client_id);
}

/// Handle client disconnections: we want to despawn every entity that was controlled by that client.
///
/// Lightyear creates one entity per client, which contains metadata associated with that client.
/// You can find that entity by calling `ConnectionManager::client_entity(client_id)`.
///
/// That client entity contains the `ControlledEntities` component, which is a set of entities that are controlled by that client.
///
/// By default, lightyear automatically despawns all the `ControlledEntities` when the client disconnects;
/// but in this example we will also do it manually to showcase how it can be done.
// Changed to observer, uses ClientEntityMap
pub(crate) fn handle_disconnected(
    trigger: Trigger<OnRemove, Connected>, // Trigger on disconnection
    mut entity_map: ResMut<ClientEntityMap>,
    mut commands: Commands,
    query: Query<&Connected>, // Query Connected component
) {
    let client_entity = trigger.target();
    // The Connected component might be gone already, but we can get the PeerId from the trigger's entity meta if needed,
    // or assume the ClientEntityMap still holds the mapping.
    // For simplicity, let's try finding the PeerId via the map.

    // Find which PeerId disconnected by searching the map (less efficient)
    let disconnected_peer_id = entity_map.0.iter()
        .find(|(_, &entity)| entity == client_entity)
        .map(|(peer_id, _)| *peer_id);

    if let Some(client_id) = disconnected_peer_id {
         debug!("Client {:?} disconnected", client_id);
         // Remove the client from the map and despawn their player entity
         if let Some(player_entity) = entity_map.0.remove(&client_id) {
             if let Some(entity_commands) = commands.get_entity(player_entity) {
                 info!("Despawning player entity {:?} for client {:?}", player_entity, client_id);
                 entity_commands.despawn();
             }
         }
    } else {
        error!("Could not find disconnected client in ClientEntityMap for entity {:?}", client_entity);
    }

    // Old logic using ControlledEntities:
    // for disconnection in disconnections.read() {
    //     debug!("Client {:?} disconnected", disconnection.client_id);
    //     if let Ok(client_entity) = manager.client_entity(disconnection.client_id) {
    //         if let Ok(controlled_entities) = client_query.get(client_entity) {
    //             for entity in controlled_entities.entities() {
    //                 commands.entity(entity).despawn();
    //             }
    //         }
    //     }
    // }
}

/// Read client inputs and move players in server therefore giving a basis for other clients
fn movement(mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>)>) {
    for (position, inputs) in position_query.iter_mut() {
        // Use current_value() for server::ActionState
        if let Some(inputs) = inputs.current_value() {
            shared::shared_movement_behaviour(position, inputs);
        }
    }
}

/// Send messages from server to clients (only in non-headless mode, because otherwise we run with minimal plugins
/// and cannot do input handling)
pub(crate) fn send_message(
    // Use ServerConnectionManager
    mut server: ResMut<ServerConnectionManager>,
    input: Option<Res<ButtonInput<KeyCode>>>,
) {
    if input.is_some_and(|input| input.pressed(KeyCode::KeyM)) {
        let message = Message1(5);
        info!("Send message: {:?}", message);
        // Use send_message_to_target and pass message by value
        server.send_message_to_target::<Channel1, Message1>(message, NetworkTarget::All)
            .unwrap_or_else(|e| {
                error!("Failed to send message: {:?}", e);
            });
    }
}
