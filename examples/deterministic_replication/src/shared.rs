use crate::protocol::*;
use avian2d::dynamics::solver::SolverConfig;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prediction::rollback::{CatchUpGated, DeterministicPredicted};
use lightyear::prelude::*;
use lightyear_avian2d::plugin::AvianReplicationMode;
use lightyear_deterministic_replication::prelude::CatchUpMode;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;
const BALL_PRESPAWN_HASH: u64 = 0xD37E_12B4_0000_0001;
const BALL_PART_PRESPAWN_HASH: u64 = 0xD37E_12B4_1000_0000;

/// SharedPlugin between the client and server.
#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.insert_resource(catch_up_mode_from_env());
        // bundles
        app.add_systems(Startup, init);

        // Visual correction uses the same history and scheduling as frame
        // interpolation, so install it in shared code for both GUI and headless
        // clients. Avian Position mode itself no longer depends on this plugin:
        // Position/Rotation remain authoritative even when it is absent.
        if !app.is_plugin_added::<FrameInterpolationPlugin>() {
            app.add_plugins(FrameInterpolationPlugin);
        }
        app.add_observer(add_frame_interpolation_components);

        // physics
        app.add_plugins(lightyear_avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
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

        crate::debug::register_debug_systems(app);
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
/// `DeterministicPredicted` entity with `Position`. The marker enables both
/// frame interpolation and the frame-history-backed visual correction pipeline.
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

#[cfg_attr(not(feature = "server"), allow(unused_variables))]
pub(crate) fn spawn_ball(
    commands: &mut Commands,
    mode: &CatchUpMode,
    is_server: bool,
    is_client: bool,
) -> Entity {
    let mut ball = commands.spawn((
        Position::default(),
        Rotation::radians(0.2),
        ColorComponent(css::AZURE.into()),
        BallBodyBundle::dynamic(),
        BallMarker,
        DeterministicPredicted {
            skip_despawn: true,
            ..default()
        },
        Name::from("Ball"),
    ));
    if *mode == CatchUpMode::StateBasedCatchUp {
        ball.insert(PreSpawned::new(BALL_PRESPAWN_HASH));
        #[cfg(feature = "server")]
        if is_server {
            ball.insert((Replicate::to_clients(NetworkTarget::All), CatchUpGated));
        } else if is_client {
            ball.insert(CatchUpGated);
        }
        #[cfg(not(feature = "server"))]
        if is_client {
            ball.insert(CatchUpGated);
        }
    }
    let ball = ball.id();
    spawn_ball_parts(commands, ball, mode, is_server, is_client);
    ball
}

#[cfg_attr(not(feature = "server"), allow(unused_variables))]
fn spawn_ball_parts(
    commands: &mut Commands,
    root: Entity,
    mode: &CatchUpMode,
    is_server: bool,
    is_client: bool,
) {
    fn finish_part(
        entity: &mut EntityCommands,
        part: BallPart,
        mode: &CatchUpMode,
        is_server: bool,
        is_client: bool,
    ) {
        entity.insert(part);
        entity.insert(DeterministicPredicted {
            skip_despawn: true,
            ..default()
        });
        if *mode == CatchUpMode::StateBasedCatchUp {
            entity.insert(PreSpawned::new(BALL_PART_PRESPAWN_HASH + part as u64));
            #[cfg(feature = "server")]
            if is_server {
                entity.insert((Replicate::to_clients(NetworkTarget::All), CatchUpGated));
            } else if is_client {
                entity.insert(CatchUpGated);
            }
            #[cfg(not(feature = "server"))]
            if is_client {
                entity.insert(CatchUpGated);
            }
        }
    }

    let mut core = commands.spawn((
        ChildOf(root),
        Transform::from_xyz(-2.0, -1.0, 0.0).with_rotation(Quat::from_rotation_z(-0.17)),
        Collider::circle(12.0),
        ColliderDensity(0.05),
        Restitution::new(1.0),
        CollisionLayers::default(),
        Name::from("BallCoreCollider"),
    ));
    finish_part(&mut core, BallPart::Core, mode, is_server, is_client);

    let mut pivot = commands.spawn((
        ChildOf(root),
        Transform::from_xyz(4.0, 2.0, 0.0).with_rotation(Quat::from_rotation_z(0.31)),
        Name::from("BallColliderPivot"),
    ));
    finish_part(&mut pivot, BallPart::Pivot, mode, is_server, is_client);
    let pivot = pivot.id();

    let mut lobe = commands.spawn((
        ChildOf(pivot),
        Transform::from_xyz(6.0, 1.0, 0.0).with_rotation(Quat::from_rotation_z(0.23)),
        Collider::circle(8.0),
        ColliderDensity(0.04),
        Restitution::new(1.0),
        CollisionLayers::from_bits(0b0010, LayerMask::ALL.0),
        Name::from("BallLobeCollider"),
    ));
    finish_part(&mut lobe, BallPart::Lobe, mode, is_server, is_client);

    let mut sensor = commands.spawn((
        ChildOf(root),
        Transform::from_xyz(1.0, 3.0, 0.0),
        Collider::circle(20.0),
        Sensor,
        CollisionEventsEnabled,
        CollisionLayers::from_bits(0b0100, LayerMask::ALL.0),
        Name::from("BallSensorCollider"),
    ));
    finish_part(&mut sensor, BallPart::Sensor, mode, is_server, is_client);
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

// Generates a pseudo-random color from the peer id.
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
