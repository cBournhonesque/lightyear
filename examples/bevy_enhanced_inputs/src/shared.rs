//! This module contains the shared code between the client and the server.
//!
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.
use crate::protocol::*;
use bevy::prelude::*;
use lightyear::prelude::PeerId;
use lightyear_examples_common::shared::SharedSettings;

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        crate::debug::register_debug_systems(app);
    }
}

pub const SHARED_SETTINGS: SharedSettings = SharedSettings {
    protocol_id: 0,
    private_key: [0; 32],
};

pub(crate) fn initial_player_position(peer_id: PeerId) -> Vec2 {
    let slot = (peer_id.to_bits() % 6) as f32;
    Vec2::new((slot - 2.5) * 90.0, 0.0)
}

pub(crate) fn color_from_id(peer_id: PeerId) -> Color {
    let h = (((peer_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    Color::hsl(h, 0.8, 0.5)
}

// Applies movement input to a player position.
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: Vec2) {
    const MOVE_SPEED: f32 = 10.0;
    position.0.y += input.y * MOVE_SPEED;
    position.0.x += input.x * MOVE_SPEED;
}
