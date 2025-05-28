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
use crate::shared::color_from_id;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::input::native::prelude::ActionState;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;
use std::sync::Arc;

#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup);
        app.add_systems(FixedUpdate, movement);
        app.add_observer(handle_connected);
        app.add_observer(handle_new_client);
        app.add_systems(Update, (transfer_authority, update_ball_color));
    }
}

fn setup(mut commands: Commands) {
    commands.spawn((
        BallMarker,
        Name::new("Ball"),
        Position(Vec2::new(300.0, 0.0)),
        Speed(Vec2::new(0.0, 1.0)),
        PlayerColor(Color::WHITE),
        Replicate::to_clients(NetworkTarget::All),
        InterpolationTarget::to_clients(NetworkTarget::All),
        // Add ReplicationSender to send updates back to clients
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
    ));
}

pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.target()).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::from("Client"),
    ));
}

/// Spawn player entity when a client connects
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    mut commands: Commands,
    query: Query<&Connected>,
) {
    let client_entity = trigger.target();
    let Ok(connected) = query.get(client_entity) else {
        return;
    };
    let client_id = connected.remote_peer_id;

    let color = color_from_id(client_id);
    let entity = commands.spawn((
        PlayerId(client_id),
        Position(Vec2::ZERO),
        PlayerColor(color),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: client_entity,
            lifetime: Lifetime::default()
        },
    ));
    info!("Create entity {:?} for client {:?}", entity.id(), client_id);
}


/// Read client inputs and move players in server therefore giving a basis for other clients
fn movement(
    mut position_query: Query<
        (&mut Position, &ActionState<Inputs>),
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

/// Assign authority over the ball to any player that comes close to it
pub(crate) fn transfer_authority(
    // timer so that we only transfer authority every X seconds
    mut timer: Local<Timer>,
    time: Res<Time>,
    // Use ServerConnectionManager (though not needed for sending messages anymore)
    // mut connection: ResMut<ServerConnectionManager>,
    mut commands: Commands,
    ball_q: Query<(Entity, &Position), With<BallMarker>>,
    player_q: Query<(&PlayerId, &Position)>, // PlayerId now contains PeerId
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
                // Use PeerId::Client for authority transfer
                commands
                    .entity(ball_entity)
                    .transfer_authority(PeerId::Client(player_id.0));

                // Removed message sending for AuthorityPeer
                // connection.send_message_to_target::<Channel1, _>(...)
                return;
            }
        }

        // if no player is close to the ball, transfer authority back to the server
        commands
            .entity(ball_entity)
            .transfer_authority(PeerId::Server); // Use PeerId::Server

        // Removed message sending for AuthorityPeer
        // connection.send_message_to_target::<Channel1, _>(...)
    }
}

/// Everytime the ball changes authority, repaint the ball according to the new owner
pub(crate) fn update_ball_color(
    // Query Authority component instead of AuthorityPeer
    mut balls: Query<(&mut PlayerColor, &Authority), (With<BallMarker>, Changed<Authority>)>,
    player_q: Query<(&PlayerId, &PlayerColor), Without<BallMarker>>, // PlayerId now contains PeerId
) {
    for (mut ball_color, authority) in balls.iter_mut() {
        info!("Ball authority changed to {:?}", authority.peer_id);
        match authority.peer_id {
            // Check authority.peer_id
            PeerId::Server => {
                // Use PeerId::Server
                ball_color.0 = Color::WHITE;
            }
            PeerId::Client(client_id) => {
                // Use PeerId::Client
                let player_color_opt = player_q
                    .iter()
                    .find(|(player_id, _)| player_id.0 == client_id)
                    .map(|(_, color)| color.0);
                if let Some(player_color) = player_color_opt {
                    ball_color.0 = player_color;
                } else {
                    warn!("Could not find player color for client {}", client_id);
                    ball_color.0 = Color::BLACK; // Fallback color
                }
            } // AuthorityPeer::None is not directly represented, absence of Authority component implies no authority
        }
    }
}
