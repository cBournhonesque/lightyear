use bevy::color::palettes::css::{BLUE, GREEN, RED};
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::action_state::ActionState;
use core::ops::Deref;

use lightyear::prelude::client::Confirmed;
use lightyear::prelude::*;
use lightyear::transport::io::IoDiagnosticsPlugin;

use crate::protocol::*;

const MOVE_SPEED: f32 = 10.0;
pub(crate) const PROP_SIZE: f32 = 5.0;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);

        // movement
        app.add_systems(FixedUpdate, player_movement);
    }
}

/// Read client inputs and move players
pub(crate) fn player_movement(
    mut position_query: Query<(&mut Position, &ActionState<Inputs>), Without<Confirmed>>,
) {
    for (mut position, input) in position_query.iter_mut() {
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
}
