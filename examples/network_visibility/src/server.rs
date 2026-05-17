use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared::{color_from_id, shared_movement_behaviour};
use bevy::prelude::*;
use lightyear::connection::client::PeerMetadata;
use lightyear::input::native::prelude::{ActionState, InputMarker};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

const GRID_SIZE: f32 = 200.0;
const NUM_CIRCLES: i32 = 1;
const INTEREST_RADIUS: f32 = 150.0;

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        app.add_plugins(RoomPlugin);
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_systems(Startup, init);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(Update, interest_management);

        // Allocate a room for the player entities
        let player_room = app.world_mut().resource_mut::<RoomAllocator>().allocate();
        app.insert_resource(PlayerRoom(player_room));
    }
}

#[derive(Resource)]
pub struct PlayerRoom(RoomId);

/// When a new client tries to connect to a server, an entity is created for it with the `ClientOf` component.
/// This entity represents the connection between the server and that client.
///
/// You can add additional components to update the connection. In this case we will add a `ReplicationSender` that
/// will enable us to replicate local entities to that client.
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

/// If the new client connects to the server, we want to spawn a new player entity for it.
///
/// We have to react specifically on `Connected` because there is no guarantee that the connection request we
/// received was valid. The server could reject the connection attempt for many reasons (server is full, packet is invalid,
/// DDoS attempt, etc.). We want to start the replication only when the client is confirmed as connected.
pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    player_room: Res<PlayerRoom>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    let color = color_from_id(client_id);
    let player_entity = commands
        .spawn((
            PlayerId(client_id),
            Position(Vec2::ZERO),
            PlayerColor(color),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: trigger.entity,
                lifetime: Default::default(),
            },
            // Add the player entity to the player room for room-based visibility
            Rooms::single(player_room.0),
        ))
        .id();
    info!(
        "Create entity {:?} for client {:?}",
        player_entity, client_id
    );

    // Add the sender (client connection) to the same room so it can see all player entities
    commands
        .entity(trigger.entity)
        .insert(Rooms::single(player_room.0));
}

pub(crate) fn init(mut commands: Commands) {
    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            commands.spawn((
                Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE)),
                CircleMarker,
                Replicate::to_clients(NetworkTarget::All),
            ));
        }
    }
}

/// Here we perform more "immediate" interest management: we will make a circle visible to a client
/// depending on the distance to the client's entity
pub(crate) fn interest_management(
    peer_metadata: Res<PeerMetadata>,
    player_query: Query<(&PlayerId, Ref<Position>), (Without<CircleMarker>, With<Replicate>)>,
    circle_query: Query<(Entity, &Position), (With<CircleMarker>, With<Replicate>)>,
    mut commands: Commands,
) {
    for (client_id, position) in player_query.iter() {
        let Some(sender_entity) = peer_metadata.mapping.get(&client_id.0) else {
            error!("Could not find sender entity for client: {:?}", client_id);
            return;
        };
        if position.is_changed() {
            // in real game, you would have a spatial index (kd-tree) to only find entities within a certain radius
            for (circle, circle_position) in circle_query.iter() {
                let distance = position.distance(**circle_position);
                if distance < INTEREST_RADIUS {
                    trace!("Gain visibility with {circle:?}");
                    commands.gain_visibility(circle, *sender_entity);
                } else {
                    trace!("Lose visibility with {circle:?}");
                    commands.lose_visibility(circle, *sender_entity);
                }
            }
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<(&mut Position, &ActionState<Inputs>), Without<InputMarker<Inputs>>>,
) {
    for (position, input) in position_query.iter_mut() {
        shared_movement_behaviour(position, input);
    }
}
