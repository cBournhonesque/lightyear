use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::input::keyboard::Key;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::inputs::leafwing::input_buffer::InputBuffer;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::Controlled;
use lightyear::shared::tick_manager;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
use crate::shared::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, connect_to_server);
        app.add_systems(
            FixedUpdate,
            // In host-server mode, the server portion is already applying the
            // character actions and so we don't want to apply the character
            // actions twice.
            handle_character_actions.run_if(not(is_host_server)),
        );
        app.add_systems(
            Update,
            (handle_new_floor, handle_new_block, handle_new_character),
        );
    }
}

/// Process character actions and apply them to their associated character
/// entity.
fn handle_character_actions(
    time: Res<Time>,
    spatial_query: SpatialQuery,
    mut query: Query<
        (
            &ActionState<CharacterAction>,
            &InputBuffer<CharacterAction>,
            CharacterQuery,
        ),
        With<Predicted>,
    >,
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
) {
    // Get the current tick. It may be a part of a rollback.
    let tick = rollback
        .as_ref()
        .map(|rb| tick_manager.tick_or_rollback_tick(rb))
        .unwrap_or(tick_manager.tick());

    for (action_state, input_buffer, mut character) in &mut query {
        // Use the current character action if it is.
        if input_buffer.get(tick).is_some() {
            apply_character_action(&time, &spatial_query, action_state, &mut character);
            continue;
        }

        // If the current character action is not real then use the last real
        // character action.
        if let Some((_, prev_action_state)) = input_buffer.get_last_with_tick() {
            apply_character_action(&time, &spatial_query, prev_action_state, &mut character);
        } else {
            // No inputs are in the buffer yet. This can happen during initial
            // connection. Apply the default input (i.e. nothing pressed).
            apply_character_action(&time, &spatial_query, action_state, &mut character);
        }
    }
}

pub(crate) fn connect_to_server(mut commands: Commands) {
    commands.connect_client();
}

/// Add physics to characters that are newly predicted. If the client controls
/// the character then add an input component.
fn handle_new_character(
    connection: Res<ClientConnection>,
    mut commands: Commands,
    mut character_query: Query<
        (Entity, &ColorComponent, Has<Controlled>),
        (Added<Predicted>, With<CharacterMarker>),
    >,
) {
    for (entity, color, is_controlled) in &mut character_query {
        if is_controlled {
            info!("Adding InputMap to controlled and predicted entity {entity:?}");
            commands.entity(entity).insert(
                InputMap::new([(CharacterAction::Jump, KeyCode::Space)])
                    .with(CharacterAction::Jump, GamepadButton::South)
                    .with(CharacterAction::Shoot, KeyCode::KeyQ)
                    .with_dual_axis(CharacterAction::Move, GamepadStick::LEFT)
                    .with_dual_axis(CharacterAction::Move, VirtualDPad::wasd()),
            );
        } else {
            info!("Remote character replicated to us: {entity:?}");
        }
        let client_id = connection.id();
        info!(?entity, ?client_id, "Adding physics to character");
        commands
            .entity(entity)
            .insert(CharacterPhysicsBundle::default());
    }
}

/// Add physics to floors that are newly replicated. The query checks for
/// replicated floors instead of predicted floors because predicted floors do
/// not exist since floors aren't predicted.
fn handle_new_floor(
    connection: Res<ClientConnection>,
    mut commands: Commands,
    character_query: Query<Entity, (Added<Replicated>, With<FloorMarker>)>,
) {
    for entity in &character_query {
        info!(?entity, "Adding physics to floor");
        commands
            .entity(entity)
            .insert(FloorPhysicsBundle::default());
    }
}

/// Add physics to blocks that are newly predicted.
fn handle_new_block(
    connection: Res<ClientConnection>,
    mut commands: Commands,
    character_query: Query<Entity, (Added<Predicted>, With<BlockMarker>)>,
) {
    for entity in &character_query {
        info!(?entity, "Adding physics to block");
        commands
            .entity(entity)
            .insert(BlockPhysicsBundle::default());
    }
}
