//! This module contains the shared code between the client and the server.
//!
//! The rendering code is here because you might want to run the example in host-server mode, where the server also acts as a client.
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.

use bevy::color::palettes; // Added for color_from_id
use bevy::prelude::*;
use std::hash::{Hash, Hasher}; // Added for PeerId hashing

use lightyear::prelude::client::Interpolated;
use lightyear::prelude::*;

use crate::protocol::*;

// Removed SharedPlugin
// #[derive(Clone)]
// pub struct SharedPlugin;
//
// impl Plugin for SharedPlugin {
//     fn build(&self, app: &mut App) {
//         app.register_type::<PlayerId>();
//         app.register_type::<PlayerColor>();
//         app.register_type::<Position>();
//         app.register_type::<Speed>();
//
//         // the protocol needs to be shared between the client and server
//         app.add_plugins(ProtocolPlugin);
//         app.add_systems(FixedUpdate, ball_movement); // ball_movement should be added by client/server plugins where needed
//     }
// }

// Moved color_from_id here from protocol.rs PlayerBundle
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    client_id.hash(&mut hasher);
    let val = hasher.finish();
    // let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0; // Old way
    let colors = [
        palettes::css::RED,
        palettes::css::GREEN,
        palettes::css::BLUE,
        palettes::css::YELLOW,
        palettes::css::CADET_BLUE,
        palettes::css::MAGENTA,
    ];
    colors[val as usize % colors.len()].into()
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<Position>, input: &Inputs) {
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

/// We move the ball only when we have authority over it.
/// The peer that has authority could be the Server, a Client or no one
pub(crate) fn ball_movement(
    mut balls: Query<
        (&mut Position, &mut Speed),
        // Query should check for HasAuthority, which is added by lightyear based on Replicate config
        (With<BallMarker>, With<HasAuthority>, Without<Interpolated>),
    >,
) {
    for (mut position, mut speed) in balls.iter_mut() {
        if position.y > 300.0 {
            speed.y = -1.0;
        }
        if position.y < -300.0 {
            speed.y = 1.0;
        }
        position.0 += speed.0;
    }
}
