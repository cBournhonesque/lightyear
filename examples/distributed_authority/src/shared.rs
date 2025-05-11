//! This module contains the shared code between the client and the server.
//!
//! The rendering code is here because you might want to run the example in host-server mode, where the server also acts as a client.
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.

// Added for color_from_id
use bevy::prelude::*;
use std::hash::Hash;
// Added for PeerId hashing

use lightyear::prelude::*;

use crate::protocol::*;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<PlayerId>();
        app.register_type::<PlayerColor>();
        app.register_type::<Position>();
        app.register_type::<Speed>();

        app.add_plugins(ProtocolPlugin);
        app.add_systems(FixedUpdate, ball_movement);
    }
}


// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
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
