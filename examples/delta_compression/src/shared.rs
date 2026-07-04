//! This module contains the shared code between the client and the server.
//!
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.

use crate::protocol::*;

// Compute the new head point for the player's trail when we receive an input.
pub(crate) fn next_trail_head(trail: &PlayerTrail, input: &Inputs) -> Option<TrailPoint> {
    const MOVE_SPEED: f32 = 10.0;
    let Inputs::Direction(direction) = input;
    if direction.is_none() {
        return None;
    }

    let mut position = trail.head();
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
    Some(TrailPoint(position))
}
