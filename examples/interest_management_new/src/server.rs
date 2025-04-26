use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::{ActionState, InputMap};

use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour};

const GRID_SIZE: f32 = 200.0;
const NUM_CIRCLES: i32 = 10;
const INTEREST_RADIUS: f32 = 150.0;

// Special room for the player entities (so that all player entities always see each other)
const PLAYER_ROOM: RoomId = RoomId(6000);

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Global>();
        app.add_systems(Startup, init);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
        app.add_systems(
            Update,
            (
                handle_connections,
                // we don't have to run interest management every tick, only every time
                // we are buffering replication messages
                interest_management.in_set(ReplicationSet::SendMessages),
                receive_message,
            ),
        );
    }
}

#[derive(Resource, Default)]
pub(crate) struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
    pub client_id_to_room_id: HashMap<ClientId, RoomId>,
}

pub(crate) fn init(mut commands: Commands) {
    commands.start_server();
    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            commands.spawn((
                Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE)),
                CircleMarker,
                Replicate {
                    // use rooms for replication
                    relevance_mode: NetworkRelevanceMode::InterestManagement,
                    ..default()
                },
            ));
        }
    }
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    mut room_manager: ResMut<RoomManager>,
    mut connections: EventReader<ConnectEvent>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.client_id;
        let entity = commands.spawn(PlayerBundle::new(client_id, Vec2::ZERO));

        // we can control the player visibility in a more static manner by using rooms
        // we add all clients to a room, as well as all player entities
        // this means that all clients will be able to see all player entities
        room_manager.add_client(client_id, PLAYER_ROOM);
        room_manager.add_entity(entity.id(), PLAYER_ROOM);
    }
}

pub(crate) fn receive_message(mut messages: EventReader<ServerReceiveMessage<Message1>>) {
    for message in messages.read() {
        info!("recv message");
    }
}

/// Here we perform more "immediate" interest management: we will make a circle visible to a client
/// depending on the distance to the client's entity
pub(crate) fn interest_management(
    mut relevance_manager: ResMut<RelevanceManager>,
    player_query: Query<
        (&PlayerId, Ref<Position>),
        (Without<CircleMarker>, With<ReplicateToClient>),
    >,
    circle_query: Query<(Entity, &Position), (With<CircleMarker>, With<ReplicateToClient>)>,
) {
    for (client_id, position) in player_query.iter() {
        if position.is_changed() {
            // in real game, you would have a spatial index (kd-tree) to only find entities within a certain radius
            for (circle_entity, circle_position) in circle_query.iter() {
                let distance = position.distance(**circle_position);
                if distance < INTEREST_RADIUS {
                    relevance_manager.gain_relevance(client_id.0, circle_entity);
                } else {
                    relevance_manager.lose_relevance(client_id.0, circle_entity);
                }
            }
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<(&mut Position, &ActionState<Inputs>), Without<InputMap<Inputs>>>,
) {
    for (position, input) in position_query.iter_mut() {
        shared_movement_behaviour(position, input);
    }
}
