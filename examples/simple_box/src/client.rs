//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//!   predicted entity and the server entity)

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared;
use bevy::prelude::*;
use lightyear::prelude::client::input::*;
use lightyear::prelude::input::native::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_automation_settings);
        app.add_systems(
            FixedPreUpdate,
            // Inputs have to be buffered in the WriteClientInputs set
            buffer_input.in_set(InputSystems::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, player_movement);

        app.add_systems(
            Update,
            (
                receive_message1,
                debug_player_entities,
                log_position_updates,
            ),
        );
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
    }
}

#[derive(Resource, Clone, Default)]
struct AutomationSettings {
    direction: Option<Direction>,
    log_positions: bool,
}

impl AutomationSettings {
    #[cfg(not(target_family = "wasm"))]
    fn from_env() -> Self {
        let direction = std::env::var("LIGHTYEAR_SIMPLE_BOX_AUTOMOVE")
            .ok()
            .and_then(|value| parse_direction(&value));
        let log_positions = std::env::var("LIGHTYEAR_SIMPLE_BOX_LOG_POSITIONS")
            .map(|value| value != "0")
            .unwrap_or(false);
        Self {
            direction,
            log_positions,
        }
    }

    #[cfg(target_family = "wasm")]
    fn from_env() -> Self {
        Self::default()
    }
}

fn init_automation_settings(mut commands: Commands) {
    let settings = AutomationSettings::from_env();
    if let Some(direction) = &settings.direction {
        info!(?direction, "Using automated client input");
    }
    if settings.log_positions {
        info!("Logging predicted and interpolated player position updates");
    }
    commands.insert_resource(settings);
}

#[cfg(not(target_family = "wasm"))]
fn parse_direction(value: &str) -> Option<Direction> {
    let mut direction = Direction::default();
    for token in value.split(',') {
        match token.trim().to_ascii_lowercase().as_str() {
            "" | "none" => {}
            "up" | "u" => direction.up = true,
            "down" | "d" => direction.down = true,
            "left" | "l" => direction.left = true,
            "right" | "r" => direction.right = true,
            other => {
                warn!(token = other, "Ignoring unknown automated input token");
            }
        }
    }
    Some(direction)
}

fn debug_player_entities(
    query: Query<
        (
            Entity,
            &PlayerId,
            Has<Predicted>,
            Has<Interpolated>,
            Has<Controlled>,
            Has<Replicated>,
        ),
        Added<PlayerId>,
    >,
) {
    for (entity, player_id, predicted, interpolated, controlled, replicated) in query.iter() {
        warn!(
            ?entity,
            ?player_id,
            predicted,
            interpolated,
            controlled,
            replicated,
            "Player entity status on client"
        );
    }
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
///
/// I would also advise to use the `leafwing` feature to use the `LeafwingInputPlugin` instead of the
/// `InputPlugin`, which contains more features.
fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    automation: Option<Res<AutomationSettings>>,
    keypress: Option<Res<ButtonInput<KeyCode>>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
        let mut direction = automation
            .as_ref()
            .and_then(|settings| settings.direction.clone())
            .unwrap_or_default();

        if direction.is_none() {
            if let Some(keypress) = keypress {
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
            }
        }
        // we always set the value. Setting it to None means that the input was missing, it's not the same
        // as saying that the input was 'no keys pressed'
        action_state.0 = Inputs::Direction(direction);
    }
}

/// The client input only gets applied to predicted entities that we own
/// This works because we only predict the user's controlled entity.
/// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    // timeline: Single<&LocalTimeline>,
    mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
) {
    // let tick = timeline.tick();
    for (position, input) in position_query.iter_mut() {
        // trace!(?tick, ?position, ?input, "client");
        // NOTE: be careful to directly pass Mut<PlayerPosition>
        // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
        shared::shared_movement_behaviour(position, input);
    }
}

/// System to receive messages on the client
pub(crate) fn receive_message1(mut receiver: Single<&mut MessageReceiver<Message1>>) {
    for message in receiver.receive() {
        info!("Received message: {:?}", message);
    }
}

fn log_position_updates(
    settings: Option<Res<AutomationSettings>>,
    query: Query<
        (
            Entity,
            &PlayerId,
            &PlayerPosition,
            Has<Predicted>,
            Has<Interpolated>,
            Has<Controlled>,
        ),
        Changed<PlayerPosition>,
    >,
) {
    let Some(settings) = settings else {
        return;
    };
    if !settings.log_positions {
        return;
    }
    for (entity, player_id, position, predicted, interpolated, controlled) in query.iter() {
        if predicted || interpolated {
            info!(
                ?entity,
                ?player_id,
                position = ?position.0,
                predicted,
                interpolated,
                controlled,
                "Player position update on client"
            );
        }
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
///
/// Note that this will be triggered multiple times: for the locally-controlled entity,
/// but also for the remote-controlled entities that are spawned with [`Interpolated`].
/// The `With<Predicted>` filter ensures we only add the `InputMarker` once.
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if let Ok(mut color) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        warn!("Add InputMarker to Predicted entity: {:?}", entity);
        commands
            .entity(entity)
            .insert(InputMarker::<Inputs>::default());
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, Interpolated>,
    mut interpolated: Query<&mut PlayerColor>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}
