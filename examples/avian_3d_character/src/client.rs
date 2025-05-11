use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::input::keyboard::Key;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
// Updated InputBuffer path
use lightyear::prelude::client::InputBuffer;
use lightyear::prelude::client::*;
use lightyear::prelude::input::InputBuffer;
// Updated Controlled path
use lightyear::prelude::Controlled;
use lightyear::prelude::*;
// Removed unused import
// use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
use crate::shared::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            // In host-server mode, the server portion is already applying the
            // character actions and so we don't want to apply the character
            // actions twice.
            handle_character_actions
                // .run_if(not(is_host_server)),
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
            &InputBuffer<ActionState<CharacterAction>>,
            CharacterQuery,
        ),
        With<Predicted>,
    >,
    timeline: Single<&LocalTimeline, With<Client>>,
) {
    let tick = timeline.tick();
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


/// Add physics to characters that are newly predicted. If the client controls
/// the character then add an input component.
fn handle_new_character(
    mut commands: Commands,
    mut character_query: Query<
        (Entity, &ColorComponent, Has<Controlled>),
        (Added<Predicted>, With<CharacterMarker>),
    >,
) {
    for (entity, _color, is_controlled) in &mut character_query {
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
            info!("Remote character predicted for us: {entity:?}");
        }
        info!(?entity, "Adding physics to character");
        commands
            .entity(entity)
            .insert(CharacterPhysicsBundle::default());
    }
}

/// Add physics to floors that are newly replicated. The query checks for
/// replicated floors instead of predicted floors because predicted floors do
/// not exist since floors aren't predicted.
fn handle_new_floor(
    mut commands: Commands,
    floor_query: Query<Entity, (Added<Replicated>, With<FloorMarker>)>,
) {
    for entity in &floor_query {
        info!(?entity, "Adding physics to floor");
        commands
            .entity(entity)
            .insert(FloorPhysicsBundle::default());
    }
}

/// Add physics to blocks that are newly predicted.
fn handle_new_block(
    mut commands: Commands,
    block_query: Query<Entity, (Added<Predicted>, With<BlockMarker>)>,
) {
    for entity in &block_query {
        info!(?entity, "Adding physics to block");
        commands
            .entity(entity)
            .insert(BlockPhysicsBundle::default());
    }
}
