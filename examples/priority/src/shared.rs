use bevy::prelude::*;
use leafwing_input_manager::action_state::ActionState;

use crate::protocol::*;

const MOVE_SPEED: f32 = 10.0;
pub(crate) const PROP_SIZE: f32 = 5.0;

// SharedPlugin is no longer needed, ProtocolPlugin is added in main.rs
#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
    }
}

/// Shared movement logic, controlled by the leafwing ActionState
pub(crate) fn shared_movement_behaviour(
    mut position: Mut<Position>,
    action_state: &ActionState<Inputs>,
) {
    if action_state.pressed(&Inputs::Up) {
        position.y += MOVE_SPEED;
    }
    if action_state.pressed(&Inputs::Down) {
        position.y -= MOVE_SPEED;
    }
    if action_state.pressed(&Inputs::Left) {
        position.x -= MOVE_SPEED;
    }
    if action_state.pressed(&Inputs::Right) {
        position.x += MOVE_SPEED;
    }
}
