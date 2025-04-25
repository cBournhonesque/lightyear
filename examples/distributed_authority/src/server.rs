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
use core::time::Duration;
use lightyear::inputs::native::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::sync::Arc;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (init, start_server));
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
        app.add_systems(
            Update,
            (transfer_authority, update_ball_color, handle_connections),
        );
    }
}

/// Start the server
fn start_server(mut commands: Commands) {
    commands.start_server();
}

/// Add some debugging text to the screen
fn init(mut commands: Commands) {
    commands.spawn((
        BallMarker,
        Name::new("Ball"),
        Position(Vec2::new(300.0, 0.0)),
        Speed(Vec2::new(0.0, 1.0)),
        PlayerColor(Color::WHITE),
        Replicate {
            sync: SyncTarget {
                interpolation: NetworkTarget::All,
                ..default()
            },
            ..default()
        },
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

/// Read client inputs and move players in server therefore giving a basis for other clients
fn movement(mut position_query: Query<(&mut Position, &ActionState<Inputs>)>) {
    for (position, inputs) in position_query.iter_mut() {
        if let Some(inputs) = &inputs.value {
            shared::shared_movement_behaviour(position, inputs);
        }
    }
}

/// Assign authority over the ball to any player that comes close to it
pub(crate) fn transfer_authority(
    // timer so that we only transfer authority every X seconds
    mut timer: Local<Timer>,
    time: Res<Time>,
    mut connection: ResMut<ConnectionManager>,
    mut commands: Commands,
    ball_q: Query<(Entity, &Position), With<BallMarker>>,
    player_q: Query<(&PlayerId, &Position)>,
) {
    if !timer.tick(time.delta()).finished() {
        return;
    }
    *timer = Timer::new(Duration::from_secs_f32(0.3), TimerMode::Once);
    for (ball_entity, ball_pos) in ball_q.iter() {
        // TODO: sort by player_id?
        for (player_id, player_pos) in player_q.iter() {
            if player_pos.0.distance(ball_pos.0) < 100.0 {
                trace!("Player {:?} has authority over the ball", player_id);
                commands
                    .entity(ball_entity)
                    .transfer_authority(AuthorityPeer::Client(player_id.0));

                // we send a message only because we want the clients to show the color
                // of the authority peer for the demo, it's not needed in practice
                connection
                    .send_message_to_target::<Channel1, _>(
                        &mut AuthorityPeer::Client(player_id.0),
                        NetworkTarget::All,
                    )
                    .unwrap();
                return;
            }
        }

        // if no player is close to the ball, transfer authority back to the server
        commands
            .entity(ball_entity)
            .transfer_authority(AuthorityPeer::Server);

        // we send a message only because we want the clients to show the color
        // of the authority peer for the demo, it's not needed in practice
        connection
            .send_message_to_target::<Channel1, _>(&mut AuthorityPeer::Server, NetworkTarget::All)
            .unwrap();
    }
}

/// Everytime the ball changes authority, repaint the ball according to the new owner
pub(crate) fn update_ball_color(
    mut balls: Query<
        (&mut PlayerColor, &AuthorityPeer),
        (With<BallMarker>, Changed<AuthorityPeer>),
    >,
    player_q: Query<(&PlayerId, &PlayerColor), Without<BallMarker>>,
) {
    for (mut ball_color, authority) in balls.iter_mut() {
        info!("Ball authority changed to {:?}", authority);
        match authority {
            AuthorityPeer::Server => {
                ball_color.0 = Color::WHITE;
            }
            AuthorityPeer::Client(client_id) => {
                for (player_id, player_color) in player_q.iter() {
                    if player_id.0 == *client_id {
                        ball_color.0 = player_color.0;
                    }
                }
            }
            AuthorityPeer::None => {
                ball_color.0 = Color::BLACK;
            }
        }
    }
}
