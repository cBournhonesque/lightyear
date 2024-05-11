//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::HashMap;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (init, start_server));
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
        app.add_systems(Update, (send_message, handle_connections));
    }
}

/// Start the server
fn start_server(mut commands: Commands) {
    commands.start_server();
}

/// Add some debugging text to the screen
fn init(mut commands: Commands) {
    commands.spawn(
        TextBundle::from_section(
            "Server",
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            align_self: AlignSelf::End,
            ..default()
        }),
    );
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    mut connections: EventReader<ConnectEvent>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.client_id;
        // server and client are running in the same app, no need to replicate to the local client
        let replicate = Replicate {
            controlled_by: NetworkTarget::Single(client_id),
            prediction_target: NetworkTarget::Single(client_id),
            interpolation_target: NetworkTarget::AllExceptSingle(client_id),
            ..default()
        };
        let entity = commands.spawn((PlayerBundle::new(client_id, Vec2::ZERO), replicate));
        info!("Create entity {:?} for client {:?}", entity.id(), client_id);
    }
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
/// (however we don't actually run the system)
pub(crate) fn handle_disconnections(
    mut commands: Commands,
    mut disconnections: EventReader<DisconnectEvent>,
    manager: Res<ConnectionManager>,
    client_query: Query<&ControlledEntities>,
) {
    for disconnection in disconnections.read() {
        debug!("Client {:?} disconnected", disconnection.client_id);
        if let Ok(client_entity) = manager.client_entity(disconnection.client_id) {
            if let Ok(controlled_entities) = client_query.get(client_entity) {
                for entity in controlled_entities.iter() {
                    commands.entity(*entity).despawn();
                }
            }
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<(&Replicate, &mut PlayerPosition)>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    tick_manager: Res<TickManager>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            trace!(
                "Receiving input: {:?} from client: {:?} on tick: {:?}",
                input,
                client_id,
                tick_manager.tick()
            );
            // NOTE: you can define a mapping from client_id to entity_id to avoid iterating through all
            //  entities here
            for (replicate, position) in position_query.iter_mut() {
                if replicate.controlled_by.targets(client_id) {
                    shared::shared_movement_behaviour(position, input);
                }
            }
        }
    }
}

/// Send messages from server to clients (only in non-headless mode, because otherwise we run with minimal plugins
/// and cannot do input handling)
pub(crate) fn send_message(
    mut server: ResMut<ConnectionManager>,
    input: Option<Res<ButtonInput<KeyCode>>>,
) {
    if input.is_some_and(|input| input.pressed(KeyCode::KeyM)) {
        let message = Message1(5);
        info!("Send message: {:?}", message);
        server
            .send_message_to_target::<Channel1, Message1>(&Message1(5), NetworkTarget::All)
            .unwrap_or_else(|e| {
                error!("Failed to send message: {:?}", e);
            });
    }
}
