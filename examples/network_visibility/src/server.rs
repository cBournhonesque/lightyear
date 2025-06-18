use crate::protocol::*;
use crate::shared::{color_from_id, shared_movement_behaviour};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use lightyear::connection::client::PeerMetadata;
use lightyear::connection::client_of::ClientOf;
use lightyear::input::native::prelude::{ActionState, InputMarker};
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

const GRID_SIZE: f32 = 200.0;
const NUM_CIRCLES: i32 = 1;
const INTEREST_RADIUS: f32 = 150.0;

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(Update, interest_management);

        // Spawn a room for the player entities
        app.world_mut().spawn(Room::default());
    }
}

#[derive(Resource)]
pub struct PlayerRoom(Entity);

/// When a new client tries to connect to a server, an entity is created for it with the `ClientOf` component.
/// This entity represents the connection between the server and that client.
///
/// You can add additional components to update the connection. In this case we will add a `ReplicationSender` that
/// will enable us to replicate local entities to that client.
pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.target())
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}

/// If the new client connnects to the server, we want to spawn a new player entity for it.
///
/// We have to react specifically on `Connected` because there is no guarantee that the connection request we
/// received was valid. The server could reject the connection attempt for many reasons (server is full, packet is invalid,
/// DDoS attempt, etc.). We want to start the replication only when the client is confirmed as connected.
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    room: Single<Entity, With<Room>>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = query.get(trigger.target()) else {
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
                owner: trigger.target(),
                lifetime: Default::default(),
            },
            // Use network visibility for interest management
            NetworkVisibility::default(),
        ))
        .id();
    info!(
        "Create entity {:?} for client {:?}",
        player_entity, client_id
    );

    // we can control the player visibility in a more static manner by using rooms
    // we add all clients to a room, as well as all player entities
    // this means that all clients will be able to see all player entities
    let room = room.into_inner();
    commands.trigger_targets(RoomEvent::AddSender(trigger.target()), room);
    commands.trigger_targets(RoomEvent::AddEntity(player_entity), room);
}

pub(crate) fn init(mut commands: Commands) {
    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            commands.spawn((
                Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE)),
                CircleMarker,
                Replicate::to_clients(NetworkTarget::All),
                // Use network visibility for interest management
                NetworkVisibility::default(),
            ));
        }
    }
}

/// Here we perform more "immediate" interest management: we will make a circle visible to a client
/// depending on the distance to the client's entity
pub(crate) fn interest_management(
    peer_metadata: Res<PeerMetadata>,
    player_query: Query<(&PlayerId, Ref<Position>), (Without<CircleMarker>, With<Replicate>)>,
    mut circle_query: Query<
        (Entity, &Position, &mut NetworkVisibility),
        (With<CircleMarker>, With<Replicate>),
    >,
) {
    for (client_id, position) in player_query.iter() {
        let Some(sender_entity) = peer_metadata.mapping.get(&client_id.0) else {
            error!("Could not find sender entity for client: {:?}", client_id);
            return;
        };
        if position.is_changed() {
            // in real game, you would have a spatial index (kd-tree) to only find entities within a certain radius
            for (circle, circle_position, mut visibility) in circle_query.iter_mut() {
                let distance = position.distance(**circle_position);
                if distance < INTEREST_RADIUS {
                    debug!("Gain visibility with {circle:?}");
                    visibility.gain_visibility(*sender_entity);
                } else {
                    debug!("Lose visibility with {circle:?}");
                    visibility.lose_visibility(*sender_entity);
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
        if let Some(input) = &input.value {
            shared_movement_behaviour(position, input);
        }
    }
}
