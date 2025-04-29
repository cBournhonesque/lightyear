use bevy::prelude::*;
use core::time::Duration;
use std::hash::{Hash, Hasher}; // Added for PeerId hashing

use lightyear::prelude::client::Confirmed;
use lightyear::prelude::*;

use crate::protocol::*;

// Removed SharedPlugin
// #[derive(Clone)]
// pub struct SharedPlugin;
//
// impl Plugin for SharedPlugin {
//     fn build(&self, app: &mut App) {
//         app.add_plugins(ProtocolPlugin);
//     }
// }

// Generate pseudo-random color from id
// Updated to use PeerId
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    client_id.hash(&mut hasher);
    let h = hasher.finish() % 360;
    // let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0; // Old way
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h as f32, s, l) // Use h as f32
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: &Inputs) {
    const MOVE_SPEED: f32 = 10.0;
    match input {
        Inputs::Direction(direction) => {
            if direction.up {
                position.y += MOVE_SPEED;
            }
            if direction.down {
                position.y -= MOVE_SPEED;
            }
            if direction.left {
                position.x -= MOVE_SPEED;
            }
            if direction.right {
                position.x += MOVE_SPEED;
            }
        }
        _ => {}
    }
}
