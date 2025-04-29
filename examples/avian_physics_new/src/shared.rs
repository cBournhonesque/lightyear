use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::ActionState;
use tracing::Level;

// Removed diagnostics plugins and common
// use lightyear::client::prediction::diagnostics::PredictionDiagnosticsPlugin;
use lightyear::prelude::client::*;
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
// use lightyear::shared::ping::diagnostics::PingDiagnosticsPlugin;
// use lightyear::transport::io::IoDiagnosticsPlugin;
// use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;
use std::hash::{Hash, Hasher}; // Added for PeerId hashing

use crate::protocol::*;
pub(crate) const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

// Removed SharedPlugin and its build method
// #[derive(Clone)]
// pub struct SharedPlugin { ... }
// impl Plugin for SharedPlugin { ... }

// Generate pseudo-random color from id
// Updated to use PeerId
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    client_id.hash(&mut hasher);
    let h = hasher.finish() % 360;
    // let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0; // Old way
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h as f32, s, l) // Use h as f32
}

// Removed init system (moved wall spawning to server setup)
// pub(crate) fn init(mut commands: Commands) { ... }

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(
    mut velocity: Mut<LinearVelocity>,
    action: &ActionState<PlayerActions>,
) {
    const MOVE_SPEED: f32 = 10.0;
    if action.pressed(&PlayerActions::Up) {
        velocity.y += MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        velocity.y -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        velocity.x -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        velocity.x += MOVE_SPEED;
    }
    *velocity = LinearVelocity(velocity.clamp_length_max(MAX_VELOCITY));
}

// Removed debug logging systems
// pub(crate) fn after_physics_log(...) { ... }
// pub(crate) fn last_log(...) { ... }
// pub(crate) fn log() { ... }

// Wall
#[derive(Bundle)]
pub(crate) struct WallBundle {
    color: ColorComponent,
    physics: PhysicsBundle,
    wall: Wall,
    name: Name,
}

#[derive(Component)]
pub(crate) struct Wall {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
}

impl WallBundle {
    pub(crate) fn new(start: Vec2, end: Vec2, color: Color) -> Self {
        Self {
            color: ColorComponent(color),
            physics: PhysicsBundle {
                collider: Collider::segment(start, end),
                collider_density: ColliderDensity(1.0),
                rigid_body: RigidBody::Static,
            },
            wall: Wall { start, end },
            name: Name::from("wall"),
        }
    }
}
