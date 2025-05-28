//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//! predicted entity and the server entity)
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;
use lightyear::input::client::InputSet;
use lightyear::input::native::prelude::{ActionState, InputMarker};
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSet::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(Update, change_ball_color_on_authority);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_ball);
    }
}

/// When a Ball entity gets replicated to use from the server, add the Replicate component
/// on the client so that we can replicate updates to the server if we get authority
/// over the ball
pub(crate) fn handle_ball(trigger: Trigger<OnAdd, BallMarker>, mut commands: Commands) {
    let mut color_override = ComponentReplicationOverrides::<PlayerColor>::default();
    color_override.global_override(ComponentReplicationOverride {
        disable: true,
        ..default()
    });
    commands.entity(trigger.target()).insert((
        Replicate::to_server(),
        Name::new("Ball"),
        // Disable PlayerColor replication from client to server
        color_override,
    ));
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
pub(crate) fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    query.iter_mut().for_each(|mut action_state| {
        let mut input = None;
        let mut direction = Direction {
            up: false,
            down: false,
            left: false,
            right: false,
        };
        if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
            direction.up = true;
        }
        if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
            direction.down = true;
        }
        if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
            direction.left = true;
        }
        if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
            direction.right = true;
        }
        if !direction.is_none() {
            input = Some(Inputs::Direction(direction));
        }
        action_state.value = input;
    });
}


fn player_movement(mut position_query: Query<(&mut Position, &ActionState<Inputs>)>) {
    for (position, input) in position_query.iter_mut() {
        if let Some(inputs) = &input.value {
            shared::shared_movement_behaviour(position, inputs);
        }
    }
}


/// Set the color of the ball to the color of the peer that has authority
// Changed to observe HasAuthority component changes
pub(crate) fn change_ball_color_on_authority(
    // Observe changes to HasAuthority on Ball entities
    authority_changes: Query<
        (Entity, &Authority),
        (
            With<BallMarker>,
            Or<(Added<HasAuthority>, Changed<HasAuthority>)>,
        ),
    >,
    no_authority_balls: Query<Entity, (With<BallMarker>, Without<HasAuthority>)>,
    players: Query<(&PlayerColor, &PlayerId), With<Confirmed>>, // Query confirmed players for color
    mut balls: Query<&mut PlayerColor, With<BallMarker>>,       // Query ball color mutably
) {
    // Handle cases where authority is gained or changed
    for (ball_entity, authority) in authority_changes.iter() {
        if let Ok(mut ball_color) = balls.get_mut(ball_entity) {
            match authority.peer_id {
                PeerId::Server => {
                    ball_color.0 = Color::WHITE;
                    info!("Ball authority changed to Server. Setting color to WHITE.");
                }
                PeerId::Client(client_id) => {
                    let player_color_opt = players
                        .iter()
                        .find(|(_, player_id)| player_id.0 == client_id)
                        .map(|(color, _)| color.0);
                    if let Some(player_color) = player_color_opt {
                        ball_color.0 = player_color;
                        info!(
                            "Ball authority changed to Client {}. Setting color.",
                            client_id
                        );
                    } else {
                        warn!("Could not find player color for client {}", client_id);
                        ball_color.0 = Color::BLACK; // Fallback color
                    }
                }
            }
        }
    }

    // Handle cases where authority is lost (HasAuthority component removed)
    // This requires tracking removals, which is harder with observers directly.
    // An alternative is to check balls without HasAuthority each frame.
    for ball_entity in no_authority_balls.iter() {
        if let Ok(mut ball_color) = balls.get_mut(ball_entity) {
            if ball_color.0 != Color::BLACK {
                // Avoid redundant sets
                ball_color.0 = Color::BLACK;
                info!("Ball lost authority. Setting color to BLACK.");
            }
        }
    }

    // Old logic using messages:
    // for event in messages.drain() { ... }
}



/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    trigger: Trigger<OnAdd, PlayerId>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
    mut commands: Commands,
) {
    let entity = trigger.target();
    if let Ok(mut color) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        warn!("Add InputMarker to entity: {:?}", entity);
        commands
            .entity(entity)
            .insert(InputMarker::<Inputs>::default());
    }
}
