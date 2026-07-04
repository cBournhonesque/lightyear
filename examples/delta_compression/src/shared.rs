//! This module contains the shared code between the client and the server.
//!
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.

use crate::protocol::*;

// This system defines how we update the player's positions when we receive an input.
pub(crate) fn shared_movement_behaviour(position: &mut PlayerPosition, input: &Inputs) -> bool {
    const MOVE_SPEED: f32 = 10.0;
    let Inputs::Direction(direction) = input;
    let mut moved = false;
    if direction.up {
        position.y += MOVE_SPEED;
        moved = true;
    }
    if direction.down {
        position.y -= MOVE_SPEED;
        moved = true;
    }
    if direction.left {
        position.x -= MOVE_SPEED;
        moved = true;
    }
    if direction.right {
        position.x += MOVE_SPEED;
        moved = true;
    }
    moved
}
