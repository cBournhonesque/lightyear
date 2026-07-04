use bevy::prelude::*;

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

/// Shared movement logic, controlled by BEI movement input.
pub(crate) fn shared_movement_behaviour(mut position: Mut<Position>, input: Vec2) {
    position.0 += input * MOVE_SPEED;
}
