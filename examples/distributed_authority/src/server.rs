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
use lightyear::connection::client::PeerMetadata;
use lightyear::connection::client_of::ClientOf;
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
    ));
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        ReplicationReceiver::default(),
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::from("Client"),
    ));
}

/// Spawn player entity when a client connects
pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    mut commands: Commands,
    query: Query<&RemoteId, With<ClientOf>>,
) {
    let Ok(client_id) = query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    let color = color_from_id(client_id);
    let entity = commands.spawn((
        PlayerId(client_id),
        Position(Vec2::ZERO),
        PlayerColor(color),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: trigger.entity,
            lifetime: Lifetime::default(),
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
        Without<Predicted>,
    >,
) {
    for (position, inputs) in position_query.iter_mut() {
        shared::shared_movement_behaviour(position, inputs);
    }
}

/// Assign authority over the ball to any player that comes close to it.
///
/// The server has the power to give the authority to any peer.
/// This is controlled via the field `has_full_control` on the [`AuthorityBroker`] component.
pub(crate) fn transfer_authority(
    // timer so that we only transfer authority every X seconds
    mut timer: Local<Timer>,
    time: Res<Time>,
    mut commands: Commands,
    ball_q: Query<(Entity, &Position), With<BallMarker>>,
    player_q: Query<(&PlayerId, &Position)>,
) {
    if !timer.tick(time.delta()).is_finished() {
        return;
    }
    *timer = Timer::new(Duration::from_secs_f32(0.3), TimerMode::Once);
    for (ball_entity, ball_pos) in ball_q.iter() {
        let mut closest = None;
        let mut closest_distance = f32::MAX;
        for (player_id, player_pos) in player_q.iter() {
            let dist = player_pos.0.distance(ball_pos.0);
            if dist < 100.0 && dist < closest_distance {
                closest_distance = dist;
                closest = Some(player_id.0);
            }
        }
        let new_authority = Some(closest.unwrap_or(PeerId::Server));
        debug!("Give authority to peer {:?}", new_authority);
        // if no player is close to the ball, transfer authority back to the server
        commands.trigger(GiveAuthority {
            entity: ball_entity,
            peer: new_authority,
        });
    }
}

/// Everytime the ball changes authority, repaint the ball according to the new owner
pub(crate) fn update_ball_color(
    broker: Query<&AuthorityBroker, (With<Server>, Changed<AuthorityBroker>)>,
    mut balls: Query<&mut PlayerColor, With<BallMarker>>,
    player_q: Query<(&PlayerId, &PlayerColor), Without<BallMarker>>, // PlayerId now contains PeerId
) {
    if let Ok(broker) = broker.single() {
        for (entity, current_authority) in broker.owners.iter() {
            if let Ok(mut color) = balls.get_mut(*entity) {
                match current_authority {
                    None => {
                        color.0 = Color::BLACK;
                    }
                    Some(PeerId::Server) => {
                        color.0 = Color::WHITE;
                    }
                    Some(p) => {
                        color.0 = color_from_id(*p);
                    }
                }
            }
        }
    }
}
