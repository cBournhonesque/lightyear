use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prelude::*;
use std::hash::{Hash, Hasher};

use crate::protocol::*;
pub(crate) const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

#[derive(Clone)]
pub struct SharedPlugin {
    pub predict_all: bool,
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        // bundles
        app.add_systems(Startup, init);

        // physics
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<ColliderHierarchyPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));

        // registry types for reflection
        app.register_type::<PlayerId>();
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
}


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
