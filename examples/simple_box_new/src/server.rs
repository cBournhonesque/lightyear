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
use lightyear_new::prelude::input::native::*;
use lightyear_new::prelude::server::*;
use lightyear_new::prelude::*;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        // the physics/FixedUpdates systems that consume inputs should be run in this set.
        // app.add_systems(FixedUpdate, movement);
        app.add_observer(handle_connections);
        // app.add_systems(Update, (send_message, handle_connections));
        #[cfg(not(feature = "client"))]
        app.add_systems(Update, server_start_stop);
    }
}

// TODO: On connection request, the user can modify the ClientOf entity that is spawned by the Serer?

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    trigger: Trigger<OnAdd, ClientConnected>,
    mut query: Query<&ClientConnected>,
    mut commands: Commands,
) {
    let connected = query.get(trigger.target()).unwrap();
    let client_id = connected.0;
    commands.entity(trigger.target()).insert(
        ReplicationSender::default(),
    );
    let entity = commands
        .spawn((
            PlayerBundle::new(client_id, Vec2::ZERO),
            // we replicate the Player entity to all clients that are connected to this server
            // TODO: the Server (and not ClientOf) should have a list of Replicate entities that are related to that server
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            // SyncTarget {
            //     prediction: NetworkTarget::Single(client_id),
            //     interpolation: NetworkTarget::AllExceptSingle(client_id),
            // },
            // ControlledBy {
            //     target: NetworkTarget::Single(client_id),
            //     ..default()
            // },
        ))
        .id();
    info!("Create entity {:?} for client {:?}", entity, client_id);
}
//
// /// Handle client disconnections: we want to despawn every entity that was controlled by that client.
// ///
// /// Lightyear creates one entity per client, which contains metadata associated with that client.
// /// You can find that entity by calling `ConnectionManager::client_entity(client_id)`.
// ///
// /// That client entity contains the `ControlledEntities` component, which is a set of entities that are controlled by that client.
// ///
// /// By default, lightyear automatically despawns all the `ControlledEntities` when the client disconnects;
// /// but in this example we will also do it manually to showcase how it can be done.
// /// (however we don't actually run the system)
// pub(crate) fn handle_disconnections(
//     mut commands: Commands,
//     mut disconnections: EventReader<DisconnectEvent>,
//     manager: Res<ConnectionManager>,
//     client_query: Query<&ControlledEntities>,
// ) {
//     for disconnection in disconnections.read() {
//         debug!("Client {:?} disconnected", disconnection.client_id);
//         if let Ok(client_entity) = manager.client_entity(disconnection.client_id) {
//             if let Ok(controlled_entities) = client_query.get(client_entity) {
//                 for entity in controlled_entities.entities() {
//                     commands.entity(entity).despawn();
//                 }
//             }
//         }
//     }
// }

/// Read client inputs and move players in server therefore giving a basis for other clients
fn movement(
    mut position_query: Query<
        (&mut PlayerPosition, &ActionState<Inputs>),
        // if we run in host-server mode, we don't want to apply this system to the local client's entities
        // because they are already moved by the client plugin
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    for (position, inputs) in position_query.iter_mut() {
        if let Some(inputs) = &inputs.value {
            shared::shared_movement_behaviour(position, inputs);
        }
    }
}

// only run this in dedicated server mode
#[cfg(not(feature = "client"))]
pub(crate) fn server_start_stop(
    mut commands: Commands,
    state: Res<State<NetworkingState>>,
    input: Option<Res<ButtonInput<KeyCode>>>,
) {
    if input.is_some_and(|input| input.just_pressed(KeyCode::KeyS)) {
        if state.get() == &NetworkingState::Stopped {
            commands.start_server();
        } else {
            commands.stop_server();
        }
    }
}

// TODO: how do we send a message to all clients of a server?
//  we could just iterate through all clients, but ideally we only serialize once, no?
//  Maybe a ServerMessageSender that serializes once, and then buffers the bytes on each Transport?

// /// Send messages from server to clients (only in non-headless mode, because otherwise we run with minimal plugins
// /// and cannot do input handling)
// pub(crate) fn send_message(
//     mut senders: Query<&mut MessageSender<Message1>>,
//     input: Option<Res<ButtonInput<KeyCode>>>,
// ) {
//     if input.is_some_and(|input| input.pressed(KeyCode::KeyM)) {
//         let message = Message1(5);
//         info!("Send message: {:?}", message);
//         server
//             .send_message_to_target::<Channel1, Message1>(&message, NetworkTarget::All)
//             .unwrap_or_else(|e| {
//                 error!("Failed to send message: {:?}", e);
//             });
//     }
// }
