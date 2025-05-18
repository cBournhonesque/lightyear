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
// Use new ActionState + InputManager paths
pub use lightyear::prelude::client::*;
use lightyear::prelude::client::{ActionState, InputManager, Replicate};
// Removed AuthorityPeer import
// use lightyear::prelude::server::AuthorityPeer;
use lightyear::prelude::*;
// Removed unused import
// use lightyear_examples_common::client_renderer::ClientIdText;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // Inputs have to be buffered in the FixedPreUpdate schedule
        app.add_systems(
            FixedPreUpdate,
            // Use new InputSystemSet path
            buffer_input.in_set(input::InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(
            Update,
            (change_ball_color_on_authority, handle_predicted_spawn),
        );
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);

        app.add_systems(PostUpdate, interpolation_debug_log);

        app.add_observer(handle_ball);
    }
}

/// When a Ball entity gets replicated to use from the server, add the Replicate component
/// on the client so that we can replicate updates to the server if we get authority
/// over the ball
pub(crate) fn handle_ball(trigger: Trigger<OnAdd, BallMarker>, mut commands: Commands) {
    commands.entity(trigger.target()).insert((
        // Add default Replicate component to mark for replication to server.
        // Server will configure detailed replication settings.
        Replicate::default(),
        // Replicate { ... } // Old complex configuration removed
        Name::new("Ball"),
        // Disable PlayerColor replication from client to server
        DisabledComponents::default().disable::<PlayerColor>(),
    ));
    // Removed remove::<HasAuthority>()
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
///
/// I would also advise to use the `leafwing` feature to use the `LeafwingInputPlugin` instead of the
/// `InputPlugin`, which contains more features.
pub(crate) fn buffer_input(
    // Use new ActionState and InputManager paths
    mut query: Query<&mut ActionState<Inputs>, With<InputManager<Inputs>>>,
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
        // Use the set() method for ActionState
        action_state.set(input);
        // action_state.value = input;
    });
}

/// The client input only gets applied to predicted entities that we own
/// This works because we only predict the user's controlled entity.
/// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(mut position_query: Query<(&mut Position, &ActionState<Inputs>)>) {
    for (position, input) in position_query.iter_mut() {
        // Use current_value() for ActionState
        if let Some(inputs) = input.current_value() {
            shared::shared_movement_behaviour(position, inputs);
        }
    }
}

/// Remove all entities when the client disconnect
fn on_disconnect(
    mut commands: Commands,
    player_entities: Query<Entity, With<PlayerId>>,
    debug_text: Query<Entity, With<ClientIdText>>,
) {
    for entity in player_entities.iter() {
        commands.entity(entity).despawn();
    }
    for entity in debug_text.iter() {
        commands.entity(entity).despawn();
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

pub(crate) fn interpolation_debug_log(
    tick_manager: Res<TickManager>,
    ball: Query<
        (
            &Position,
            &InterpolateStatus<Position>,
            &ConfirmedHistory<Position>,
        ),
        (With<BallMarker>, Without<Confirmed>),
    >,
) {
    let tick = tick_manager.tick();
    for (position, status, history) in ball.iter() {
        trace!(?tick, ?position, ?status, ?history, "Interpolation");
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    mut predicted: Query<(Entity, &mut PlayerColor), Added<Predicted>>,
    mut commands: Commands,
) {
    for (entity, mut color) in predicted.iter_mut() {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        commands
            .entity(entity)
            // Use new InputManager path
            .insert(InputManager::<Inputs>::default());
    }
}
