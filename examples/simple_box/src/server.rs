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
use bevy::prelude::*;
use bevy::utils::HashMap;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::ReplicationTarget;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunksCount>();
        app.add_systems(Startup, (init, start_server));
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
        app.add_systems(Update, (receive_message, handle_connections));
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

    commands.spawn((
        // Here we are able to call the `From` method instead of creating a new `TextSection`.
        // This will use the default font (a minimal subset of FiraMono) and apply the default styling.
        TextBundle::from("Chunks Received: 0").with_style(Style {
            position_type: PositionType::Absolute,
            bottom: Val::Px(50.0),
            left: Val::Px(10.0),
            ..default()
        }),
        ChunksTextMarker,
    ));
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
            sync: SyncTarget {
                prediction: NetworkTarget::Single(client_id),
                interpolation: NetworkTarget::AllExceptSingle(client_id),
            },
            controlled_by: ControlledBy {
                target: NetworkTarget::Single(client_id),
                ..default()
            },
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
                for entity in controlled_entities.entities() {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<(&ControlledBy, &mut PlayerPosition)>,
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
            for (controlled_by, position) in position_query.iter_mut() {
                if controlled_by.targets(client_id) {
                    shared::shared_movement_behaviour(position, input);
                }
            }
        }
    }
}

#[derive(Component)]
pub(crate) struct ChunksTextMarker;

#[derive(Default, Resource)]
pub(crate) struct ChunksCount(usize);

/// System to receive messages on the client
pub(crate) fn receive_message(
    mut reader: EventReader<MessageEvent<ChunkUpdate>>,
    mut text: Query<&mut Text, With<ChunksTextMarker>>,
    mut res: ResMut<ChunksCount>,
) {
    let mut text = text.single_mut();
    for event in reader.read() {
        res.0 += 1;
        text.sections[0].value = format!("Chunks Received: {}", res.0);
        println!(
            "Received chunk at {}",
            UNIX_EPOCH.elapsed().unwrap().as_millis()
        );
    }
}
