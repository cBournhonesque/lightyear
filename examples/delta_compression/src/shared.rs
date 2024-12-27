//! This module contains the shared code between the client and the server.
//!
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.
use bevy::prelude::*;
use bevy::utils::Duration;

use lightyear::prelude::*;
use lightyear::shared::config::Mode;

use crate::protocol::*;

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: &Inputs) {
    const MOVE_SPEED: f32 = 10.0;
    if let Inputs::Direction(direction) = input {
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
}
