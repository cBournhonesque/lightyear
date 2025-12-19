use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::input::keyboard::Key;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::Controlled;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, handle_character_actions);
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
        (Entity, &ComputedMass, &ActionState<CharacterAction>, Forces),
        With<Predicted>,
    >,
    // In host-server mode, the server portion is already applying the
    // character actions and so we don't want to apply the character
    // actions twice. This excludes host-server mode since there are multiple timelines
    // when running in host-server mode.
    timeline: Res<LocalTimeline>,
) {
    let tick = timeline.tick();
    for (entity, computed_mass, action_state, forces) in &mut query {
        // lightyear handles correctly both inputs from the local player or the remote player, during rollback
        // or out of rollback.
        // The ActionState is always updated to contain the correct action for the current tick.
        //
        // For remote players, we use the most recent input received
        apply_character_action(
            entity,
            computed_mass,
            &time,
            &spatial_query,
            action_state,
            forces,
        );
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
