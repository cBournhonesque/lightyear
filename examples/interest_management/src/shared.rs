use bevy::color::palettes::css::GREEN;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::action_state::ActionState;
use core::ops::Deref;

use lightyear::client::components::Confirmed;
use lightyear::prelude::*;

use crate::protocol::*;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<Position>, input: &ActionState<Inputs>) {
    const MOVE_SPEED: f32 = 10.0;
    if input.pressed(&Inputs::Up) {
        position.y += MOVE_SPEED;
    }
    if input.pressed(&Inputs::Down) {
        position.y -= MOVE_SPEED;
    }
    if input.pressed(&Inputs::Left) {
        position.x -= MOVE_SPEED;
    }
    if input.pressed(&Inputs::Right) {
        position.x += MOVE_SPEED;
    }
}

/// Generate a color from the `ClientId`
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}
