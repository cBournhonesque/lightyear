use crate::protocol::*;
use avian2d::dynamics::solver::SolverConfig;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::rollback::{CatchUpGated, DeterministicPredicted};
use lightyear::prelude::*;
use lightyear_avian2d::plugin::AvianReplicationMode;
use lightyear_deterministic_replication::prelude::CatchUpMode;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;
const BALL_PRESPAWN_HASH: u64 = 0xD37E_12B4_0000_0001;

/// SharedPlugin between the client and server.
#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.insert_resource(catch_up_mode_from_env());
        // bundles
        app.add_systems(Startup, init);

        // Frame interpolation on Position/Rotation. Even without a GUI, we
        // need this for its `FrameInterpolationSystems::Restore` system to
        // run in `RunFixedMainLoop` BEFORE Avian's `transform_to_position`
        // sync — otherwise the post-rollback Position gets overwritten by
        // the (stale) Transform at the start of each FixedUpdate. See the
        // TODO in `crates/integration/avian/src/plugin.rs::AvianReplicationMode::Position`.
        app.add_plugins(FrameInterpolationPlugin);
        app.add_observer(add_frame_interpolation_components);

        // physics
        app.add_plugins(lightyear_avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position,
            rollback_resources: true,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .with_length_unit(10.0)
                .build()
                // disable syncing position<>transform as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                // interpolation is handled by lightyear_frame_interpolation
                .disable::<PhysicsInterpolationPlugin>()
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO))
        .insert_resource(SolverConfig {
            warm_start_coefficient: 0.0,
            ..default()
        });
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

fn catch_up_mode_from_env() -> CatchUpMode {
    #[cfg(not(target_family = "wasm"))]
    {
        match std::env::var("LIGHTYEAR_CATCHUP_MODE") {
            Ok(value)
                if matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "input-only" | "input_only" | "inputonly" | "input"
                ) =>
            {
                CatchUpMode::InputOnly
            }
            _ => CatchUpMode::StateBasedCatchUp,
        }
    }
    #[cfg(target_family = "wasm")]
    {
        CatchUpMode::StateBasedCatchUp
    }
}

/// Insert `FrameInterpolate` on any
/// `DeterministicPredicted` entity with `Position`. Required so that
/// `FrameInterpolationSystems::Restore` runs BEFORE Avian's transform→position
/// sync at each FixedUpdate iteration, preserving post-rollback Position.
///
/// Triggered on `DeterministicPredicted` add (not `Position` add) because on
/// the client, catch-up gated entities already have `Position` when
/// `DeterministicPredicted` is inserted — so an `On<Add, Position>` observer
/// would miss them.
fn add_frame_interpolation_components(
    trigger: On<Add, DeterministicPredicted>,
    query: Query<(), (With<Position>, Without<FrameInterpolate>)>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(FrameInterpolate);
    }
}

pub(crate) fn init(
    mut commands: Commands,
    mode: Res<CatchUpMode>,
    server: Option<Single<(), With<Server>>>,
    client: Option<Single<(), With<Client>>>,
) {
    let is_server = server.is_some();
    let is_client = client.is_some();

    spawn_ball(&mut commands, &mode, is_server, is_client);
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

pub(crate) fn spawn_ball(
    commands: &mut Commands,
    mode: &CatchUpMode,
    is_server: bool,
    is_client: bool,
) -> Entity {
    let mut ball = commands.spawn((
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
    if *mode == CatchUpMode::StateBasedCatchUp {
        ball.insert(PreSpawned::new(BALL_PRESPAWN_HASH));
        if is_server {
            ball.insert((Replicate::to_clients(NetworkTarget::All), CatchUpGated));
        } else if is_client {
            ball.insert(CatchUpGated);
        }
    }
    ball.id()
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
    timeline: Res<LocalTimeline>,
    mut players: Query<
        (
            &mut LinearVelocity,
            &ActionState<PlayerActions>,
            Option<&PlayerActivationTick>,
        ),
        With<PlayerId>,
    >,
) {
    let tick = timeline.tick();
    for (velocity, action_state, activation_tick) in players.iter_mut() {
        if activation_tick.is_some_and(|activation_tick| tick < activation_tick.0) {
            continue;
        }
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
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            Option<&FrameInterpolate>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
            Option<&PlayerId>,
            Has<BallMarker>,
        ),
        Or<(With<PlayerId>, With<BallMarker>)>,
    >,
) {
    let tick = timeline.tick();
    for (
        entity,
        position,
        rotation,
        linear_velocity,
        angular_velocity,
        interpolate,
        correction,
        action_state,
        input_buffer,
        player_id,
        is_ball,
    ) in players.iter()
    {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedUpdateBeforePhysics,
            "FixedPostUpdate",
            "player_before_physics",
            tick = ?tick,
            entity = ?entity,
            player_id = ?player_id,
            is_ball,
            position = ?position,
            rotation = ?rotation,
            linear_velocity = ?linear_velocity,
            angular_velocity = ?angular_velocity,
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
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            Option<&FrameInterpolate>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&LeafwingBuffer<PlayerActions>>,
            Option<&PlayerId>,
            Has<BallMarker>,
        ),
        Or<(With<PlayerId>, With<BallMarker>)>,
    >,
) {
    let tick = timeline.tick();
    for (
        entity,
        position,
        rotation,
        linear_velocity,
        angular_velocity,
        interpolate,
        correction,
        action_state,
        input_buffer,
        player_id,
        is_ball,
    ) in players.iter()
    {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "player_fixed_last",
            tick = ?tick,
            entity = ?entity,
            player_id = ?player_id,
            is_ball,
            position = ?position,
            rotation = ?rotation,
            linear_velocity = ?linear_velocity,
            angular_velocity = ?angular_velocity,
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
    collision: CollisionMargin,
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
                restitution: Restitution::new(1.0),
            },
            collision: CollisionMargin(2.0),
            wall: Wall { start, end },
            name: Name::from("Wall"),
        }
    }
}
