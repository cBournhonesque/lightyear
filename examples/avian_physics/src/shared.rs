use crate::protocol::*;
use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client_of::ClientOf;
use lightyear::input::input_buffer::InputBuffer;
use lightyear::prelude::*;
use std::hash::{Hash, Hasher};

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
        app.add_plugins(PhysicsPlugins::default())
            .insert_resource(Gravity(Vec2::ZERO));

        app.add_systems(FixedLast, after_physics_log);

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
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(
    mut velocity: Mut<LinearVelocity>,
    action: &ActionState<PlayerActions>,
) {
    info!(pressed = ?action.get_pressed(), "shared movement");
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

pub(crate) fn after_physics_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Or<(With<Client>, Without<ClientOf>)>>,
    players: Query<
        (Entity, &Position, &Rotation, &ActionState<PlayerActions>),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<(&Position, &Rotation), (With<BallMarker>, Without<Confirmed>)>,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();

    for (entity, position, rotation, action_state) in players.iter() {
        info!(
            ?rollback,
            ?tick,
            ?entity,
            ?position,
            rotation = ?rotation.as_degrees(),
            actions = ?action_state.get_pressed(),
            "Player after physics update"
        );
    }
    // for (position, rotation) in ball.iter() {
    //     info!(?tick, ?position, ?rotation, "Ball after physics update");
    // }
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
