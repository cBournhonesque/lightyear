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
use bevy::utils::Duration;
use lightyear::client::input::native::InputSystemSet;
pub use lightyear::prelude::client::*;
use lightyear::prelude::server::AuthorityPeer;
use lightyear::prelude::*;
use lightyear_examples_common::client_renderer::ClientIdText;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // Inputs have to be buffered in the FixedPreUpdate schedule
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(Update, change_ball_color_on_authority);
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);

        app.add_systems(PostUpdate, interpolation_debug_log);

        app.add_observer(handle_ball);
    }
}

/// When a Ball entity gets replicated to use from the server, add the Replicate component
/// on the client so that we can replicate updates to the server if we get authority
/// over the ball
pub(crate) fn handle_ball(trigger: Trigger<OnAdd, BallMarker>, mut commands: Commands) {
    commands
        .entity(trigger.entity())
        .insert((
            Replicate::default(),
            Name::new("Ball"),
            DisabledComponents::default().disable::<PlayerColor>(),
        ))
        // NOTE: we need to make sure that the ball doesn't have authority!
        //  or should let the client receive updates even if it has HasAuthority
        .remove::<HasAuthority>();
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
///
/// I would also advise to use the `leafwing` feature to use the `LeafwingInputPlugin` instead of the
/// `InputPlugin`, which contains more features.
pub(crate) fn buffer_input(
    tick_manager: Res<TickManager>,
    mut input_manager: ResMut<InputManager<Inputs>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    let tick = tick_manager.tick();
    let mut input = Inputs::None;
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
        input = Inputs::Direction(direction);
    }
    if keypress.pressed(KeyCode::Backspace) {
        input = Inputs::Delete;
    }
    if keypress.pressed(KeyCode::Space) {
        input = Inputs::Spawn;
    }
    input_manager.add_input(input, tick)
}

/// The client input only gets applied to predicted entities that we own
/// This works because we only predict the user's controlled entity.
/// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    mut position_query: Query<&mut Position, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            for position in position_query.iter_mut() {
                shared::shared_movement_behaviour(position, input);
            }
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
        commands.entity(entity).despawn_recursive();
    }
    for entity in debug_text.iter() {
        commands.entity(entity).despawn_recursive();
    }
}

/// Set the color of the ball to the color of the peer that has authority
pub(crate) fn change_ball_color_on_authority(
    mut messages: ResMut<Events<MessageEvent<AuthorityPeer>>>,
    players: Query<(&PlayerColor, &PlayerId), With<Confirmed>>,
    mut balls: Query<&mut PlayerColor, (With<BallMarker>, Without<PlayerId>, With<Interpolated>)>,
) {
    for event in messages.drain() {
        if let Ok(mut ball_color) = balls.get_single_mut() {
            match event.message {
                AuthorityPeer::Server => {
                    ball_color.0 = Color::WHITE;
                }
                AuthorityPeer::Client(client_id) => {
                    for (player_color, player_id) in players.iter() {
                        if player_id.0 == client_id {
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
