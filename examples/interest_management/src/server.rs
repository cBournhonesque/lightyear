use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::prelude::ActionState;

pub use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{shared, ServerTransports, SharedSettings};

const GRID_SIZE: f32 = 200.0;
const NUM_CIRCLES: i32 = 10;
const INTEREST_RADIUS: f32 = 200.0;

// Special room for the player entities (so that all player entities always see each other)
const PLAYER_ROOM: RoomId = RoomId(6000);

// Plugin group to add all server-related plugins
pub struct ServerPluginGroup {
    pub(crate) lightyear: ServerPlugin<MyProtocol>,
}

impl ServerPluginGroup {
    pub(crate) fn new(net_configs: Vec<NetConfig>) -> ServerPluginGroup {
        let config = ServerConfig {
            shared: shared_config(),
            net: net_configs,
            ..default()
        };
        let plugin_config = PluginConfig::new(config, protocol());
        ServerPluginGroup {
            lightyear: ServerPlugin::new(plugin_config),
        }
    }
}

impl PluginGroup for ServerPluginGroup {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(self.lightyear)
            .add(ExampleServerPlugin)
            .add(shared::SharedPlugin)
            .add(LeafwingInputPlugin::<MyProtocol, Inputs>::default())
    }
}

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
                interest_management,
                log,
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
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Server",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));

    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            commands.spawn((
                Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE)),
                CircleMarker,
                Replicate {
                    // use rooms for replication
                    replication_mode: ReplicationMode::Room,
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
    mut disconnections: EventReader<DisconnectEvent>,
    mut global: ResMut<Global>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.context();
        // Generate pseudo random color from client id.
        let h = (((client_id.wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let entity = commands.spawn(PlayerBundle::new(
            *client_id,
            Vec2::ZERO,
            Color::hsl(h, s, l),
        ));
        // Add a mapping from client id to entity id (so that when we receive an input from a client,
        // we know which entity to move)
        global
            .client_id_to_entity_id
            .insert(*client_id, entity.id());
        // we will create a room for each client. To keep things simple, the room id will be the client id
        let room_id = RoomId((*client_id) as u16);
        room_manager.room_mut(room_id).add_client(*client_id);
        room_manager.room_mut(PLAYER_ROOM).add_client(*client_id);
        // also add the player entity to that room (so that the client can always see their own player)
        room_manager.room_mut(room_id).add_entity(entity.id());
        room_manager.room_mut(PLAYER_ROOM).add_entity(entity.id());
    }
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        if let Some(entity) = global.client_id_to_entity_id.remove(client_id) {
            commands.entity(entity).despawn();
        }
    }
}

pub(crate) fn log(tick_manager: Res<TickManager>, position: Query<&Position, With<PlayerId>>) {
    let server_tick = tick_manager.tick();
    for pos in position.iter() {
        debug!(?server_tick, "Confirmed position: {:?}", pos);
    }
}

pub(crate) fn receive_message(mut messages: EventReader<MessageEvent<Message1>>) {
    for message in messages.read() {
        info!("recv message");
    }
}

/// This is where we perform scope management:
/// - we will add/remove other entities from the player's room only if they are close
pub(crate) fn interest_management(
    mut room_manager: ResMut<RoomManager>,
    player_query: Query<(&PlayerId, Ref<Position>), Without<CircleMarker>>,
    circle_query: Query<(Entity, &Position), With<CircleMarker>>,
) {
    for (client_id, position) in player_query.iter() {
        if position.is_changed() {
            let room_id = RoomId((client_id.0) as u16);
            // let circles_in_room = server.room(room_id).entities();
            let mut room = room_manager.room_mut(room_id);
            for (circle_entity, circle_position) in circle_query.iter() {
                let distance = position.distance(**circle_position);
                if distance < INTEREST_RADIUS {
                    // add the circle to the player's room
                    room.add_entity(circle_entity)
                } else {
                    // if circles_in_room.contains(&circle_entity) {
                    room.remove_entity(circle_entity);
                    // }
                }
            }
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(mut position_query: Query<(&mut Position, &ActionState<Inputs>)>) {
    for (position, input) in position_query.iter_mut() {
        shared_movement_behaviour(position, input);
    }
}
