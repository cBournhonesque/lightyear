use crate::protocol::*;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_avian2d::plugin::AvianReplicationMode;
use lightyear_frame_interpolation::FrameInterpolate;

const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

/// Controls when player physics are activated in deterministic mode.
#[derive(Resource, Clone, Debug, PartialEq)]
pub enum GameStartMode {
    /// Wait until `num_players` players have connected and sent their first
    /// input, then activate physics for everyone at the same tick.
    AllReady { num_players: usize },
    /// Activate each player's physics individually as soon as their first
    /// input is received. Allows late-joining.
    Flexible,
}

impl Default for GameStartMode {
    fn default() -> Self {
        match std::env::var("LIGHTYEAR_GAME_MODE").as_deref() {
            Ok("flexible") => GameStartMode::Flexible,
            _ => GameStartMode::AllReady { num_players: 2 },
        }
    }
}

/// SharedPlugin between the client and server.
#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameStartMode>();
        app.add_plugins(ProtocolPlugin);
        // bundles
        app.add_systems(Startup, init);

        // physics
        app.add_plugins(lightyear_avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position,
            rollback_resources: true,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable syncing position<>transform as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                // interpolation is handled by lightyear_frame_interpolation
                .disable::<PhysicsInterpolationPlugin>()
                // disable island sleeping plugin as it's not compatible with rollbacks
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));

        // Game logic
        app.add_systems(FixedUpdate, player_movement);

        // Structured debug events for offline investigation via LIGHTYEAR_DEBUG_FILE.
        app.add_systems(
            FixedPostUpdate,
            emit_before_physics
                .after(PhysicsSystems::Prepare)
                .before(PhysicsSystems::StepSimulation),
        );
        app.add_systems(FixedLast, emit_fixed_last_players);
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn((
        Position::default(),
        ColorComponent(css::AZURE.into()),
        PhysicsBundle::ball(),
        BallMarker,
        DeterministicPredicted {
            skip_despawn: true,
            ..default()
        },
        Name::from("Ball"),
    ));
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

pub(crate) fn player_bundle(peer_id: PeerId) -> impl Bundle {
    let color = color_from_id(peer_id);
    let y = (peer_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    (
        Position::from(Vec2::new(-50.0, y)),
        ColorComponent(color),
        PhysicsBundle::player(),
        Name::from("Player"),
    )
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

fn shared_movement_behaviour(
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

/// In deterministic replication, every peer simulates every player.
fn player_movement(
    mut players: Query<(&mut LinearVelocity, &ActionState<PlayerActions>), With<PlayerId>>,
) {
    for (velocity, action_state) in players.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            shared_movement_behaviour(velocity, action_state);
        }
    }
}

pub(crate) fn emit_before_physics(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            Option<&FrameInterpolate<Position>>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
        ),
        (Without<BallMarker>, With<PlayerId>),
    >,
) {
    let tick = timeline.tick();
    for (entity, position, interpolate, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedUpdateBeforePhysics,
            "FixedPostUpdate",
            "player_before_physics",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            interpolate = ?interpolate,
            correction = ?correction,
            pressed = ?pressed,
            last_buffer_tick = ?last_buffer_tick,
            "Player right before Physics::StepSimulation"
        );
    }
}

pub(crate) fn emit_fixed_last_players(
    timeline: Res<LocalTimeline>,
    players: Query<
        (
            Entity,
            &Position,
            Option<&FrameInterpolate<Position>>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
        ),
        (Without<BallMarker>, With<PlayerId>),
    >,
) {
    let tick = timeline.tick();
    for (entity, position, interpolate, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "player_fixed_last",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            interpolate = ?interpolate,
            correction = ?correction,
            pressed = ?pressed,
            last_buffer_tick = ?last_buffer_tick,
            "Player in FixedLast"
        );
    }
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
                restitution: Restitution::new(0.0),
            },
            wall: Wall { start, end },
            name: Name::from("Wall"),
        }
    }
}
