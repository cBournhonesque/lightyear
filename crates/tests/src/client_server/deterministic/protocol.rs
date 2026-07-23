//! Protocol used by the deterministic-replication integration tests.
//!
//! Uses BEI (bevy_enhanced_input) for inputs: a single `DetMovement` action
//! with an `Axis2D` output (Vec2) driving the player's acceleration, plus
//! a `Player` context component on every player entity.
//!
//! Physics uses Avian2D in
//! `AvianReplicationMode::Position { sync_to_transform: false }`. A ball + walls are spawned
//! locally on every peer as a deterministic init state.

use avian2d::prelude::*;
use avian2d::{
    collision::contact_types::ContactGraph,
    dynamics::solver::{constraint_graph::ConstraintGraph, islands::PhysicsIslands},
};
use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};
use lightyear::input::bei::prelude::{BEIBuffer, BEIStateSequence};
use lightyear::input::config::InputConfig;
use lightyear::prelude::input::InputRegistryExt;
use lightyear::prelude::input::bei;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{
    AppCatchUpExt, CatchUpMode, ChecksumPlugin, LateJoinCatchUpPlugin,
};
use lightyear_prediction::rollback::{CatchUpGated, DeterministicPredicted};
use serde::{Deserialize, Serialize};

pub const PLAYER_SIZE: f32 = 20.0;
pub const BALL_SIZE: f32 = 8.0;
/// Small box so players + ball collide frequently → determinism is rigorously tested.
pub const WALL_HALF_EXTENT: f32 = 80.0;
pub const MOVE_ACCEL: f32 = 12.0;
pub const MAX_VELOCITY: f32 = 120.0;
const BALL_PRESPAWN_HASH: u64 = 0xD37E_12B4_0000_0002;
const BALL_PART_PRESPAWN_HASH: u64 = 0xD37E_12B4_1000_0000;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct DetPlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct DetPlayerActivationTick(pub Tick);

impl DetPlayerActivationTick {
    pub const DELAY_TICKS: u32 = 30;

    pub fn pending() -> Self {
        Self(Tick(u32::MAX))
    }

    pub fn is_pending(&self) -> bool {
        self.0 == Tick(u32::MAX)
    }
}

#[derive(Component, Debug)]
pub struct DetBallMarker;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetBallPart {
    Core,
    Pivot,
    Lobe,
    Sensor,
}

#[derive(Component, Debug)]
pub struct DetWallMarker;

/// Input context component added to each player entity.
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct Player;

/// Axis2D movement action — x,y in [-1, 1].
#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct DetMovement;

#[derive(Bundle)]
pub struct DetPhysicsBundle {
    pub collider: Collider,
    pub collider_density: ColliderDensity,
    pub rigid_body: RigidBody,
    pub restitution: Restitution,
}

impl DetPhysicsBundle {
    pub fn ball() -> Self {
        Self {
            collider: Collider::circle(BALL_SIZE),
            collider_density: ColliderDensity(0.05),
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(0.8),
        }
    }

    pub fn player() -> Self {
        Self {
            collider: Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
            collider_density: ColliderDensity(0.2),
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(0.5),
        }
    }
}

#[derive(Bundle)]
struct DetBallBodyBundle {
    rigid_body: RigidBody,
}

impl DetBallBodyBundle {
    fn dynamic() -> Self {
        Self {
            rigid_body: RigidBody::Dynamic,
        }
    }
}

/// Shared between server and clients — registers everything needed for
/// deterministic replication: physics, BEI inputs, catch-up,
/// frame-interpolation on Position/Rotation (required to preserve
/// post-rollback state under
/// `AvianReplicationMode::Position { sync_to_transform: false }`).
#[derive(Clone, Copy, Debug)]
pub struct DetProtocolPlugin {
    pub enable_islands: bool,
    pub compound_ball: bool,
}

impl Default for DetProtocolPlugin {
    fn default() -> Self {
        Self {
            enable_islands: false,
            compound_ball: false,
        }
    }
}

impl Plugin for DetProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CompoundBallFixture(self.compound_ball));
        app.add_plugins(bei::InputPlugin::<Player>::new(InputConfig::<Player> {
            rebroadcast_inputs: true,
            ..default()
        }));
        app.register_input_action::<DetMovement>();

        app.add_plugins(ChecksumPlugin);
        app.add_plugins(LateJoinCatchUpPlugin);
        app.register_catchup_filter::<
            (Position, Rotation, LinearVelocity, AngularVelocity),
            BEIStateSequence<Player>,
        >();
        register_avian_catchup_resources(app, self.enable_islands);

        app.add_plugins(lightyear_avian2d::plugin::LightyearAvianPlugin {
            replication_mode: lightyear_avian2d::plugin::AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            register_physics_components: false,
            rollback_resources: true,
            ..default()
        });
        let mut physics_plugins = PhysicsPlugins::default()
            .build()
            .disable::<PhysicsTransformPlugin>()
            .disable::<PhysicsInterpolationPlugin>();
        if !self.enable_islands {
            physics_plugins = physics_plugins
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>();
        }
        app.add_plugins(physics_plugins);
        app.insert_resource(Gravity(Vec2::ZERO));

        // Keep frame interpolation in shared code so rendered and headless clients use the same
        // frame-history and correction pipeline. Position mode itself no longer depends on this
        // restore to remain authoritative; add_correction below also guarantees that the plugin is
        // installed if this explicit setup is removed later.
        if !app.is_plugin_added::<FrameInterpolationPlugin>() {
            app.add_plugins(FrameInterpolationPlugin);
        }
        app.add_observer(add_frame_interpolation);

        app.component::<DetPlayerId>().replicate();
        app.component::<DetPlayerActivationTick>().replicate();

        app.component::<Position>().replicate_once();
        app.interpolate_with::<Position>(InterpolationFns::no_history(|start, end, t| {
            lightyear_avian2d::types::position::lerp(&start, &end, t)
        }));
        app.local_rollback::<Position>()
            .add_confirmed_write()
            .into_component_registration()
            .add_custom_hash(lightyear_avian2d::types::position::hash)
            .add_correction();

        app.component::<Rotation>().replicate_once();
        app.interpolate_with::<Rotation>(InterpolationFns::no_history(|start, end, t| {
            lightyear_avian2d::types::rotation::lerp(&start, &end, t)
        }));
        app.local_rollback::<Rotation>()
            .add_confirmed_write()
            .into_component_registration()
            .add_custom_hash(lightyear_avian2d::types::rotation::hash)
            .add_correction();

        app.component::<LinearVelocity>().replicate_once();
        app.local_rollback::<LinearVelocity>().add_confirmed_write();

        app.component::<AngularVelocity>().replicate_once();
        app.local_rollback::<AngularVelocity>()
            .add_confirmed_write();

        // Apply movement via a Fire observer on the action.
        app.add_observer(apply_movement);
        app.add_systems(PreUpdate, update_player_activation_ticks);

        app.add_systems(Startup, init_shared);
    }
}

#[derive(Resource)]
struct CompoundBallFixture(bool);

fn register_avian_catchup_resources(app: &mut App, enable_islands: bool) {
    app.register_catchup::<ContactGraph, BEIStateSequence<Player>>();
    app.register_catchup::<ConstraintGraph, BEIStateSequence<Player>>();
    app.register_required_components::<ContactGraph, CatchUpGated>();
    app.register_required_components::<ConstraintGraph, CatchUpGated>();
    if enable_islands {
        app.register_catchup::<PhysicsIslands, BEIStateSequence<Player>>();
        app.register_required_components::<PhysicsIslands, CatchUpGated>();
    }
}

fn update_player_activation_ticks(
    server: Option<Single<(), With<Server>>>,
    timeline: Res<LocalTimeline>,
    mut players: Query<(Entity, &DetPlayerId, &mut DetPlayerActivationTick)>,
    actions: Query<(&ActionOf<Player>, &DetBuffer)>,
) {
    use bevy::ecs::relationship::Relationship;

    if server.is_none() {
        return;
    }
    let current_tick = timeline.tick();
    for (player_entity, player_id, mut activation_tick) in &mut players {
        if !activation_tick.is_pending() {
            continue;
        }
        let ready = actions.iter().any(|(action_of, buffer)| {
            action_of.get() == player_entity
                && matches!(buffer.last_remote_tick, Some(tick) if tick >= current_tick)
        });
        if !ready {
            continue;
        }
        activation_tick.0 = current_tick + DetPlayerActivationTick::DELAY_TICKS as i32;
        info!(
            player_id = ?player_id.0,
            ?current_tick,
            activation_tick = ?activation_tick.0,
            "Activating deterministic test player after input rebroadcast warmup"
        );
    }
}

/// Spawn the ball + walls on every peer. These are NOT replicated —
/// deterministic init means identical starting state on both sides.
fn init_shared(
    mut commands: Commands,
    mode: Res<CatchUpMode>,
    compound_ball: Res<CompoundBallFixture>,
    server: Option<Single<(), With<Server>>>,
    client: Option<Single<(), With<Client>>>,
) {
    let is_state_based = *mode == CatchUpMode::StateBasedCatchUp;
    let is_server = server.is_some();
    let is_client = client.is_some();

    let mut ball = commands.spawn((
        Position::default(),
        DetBallMarker,
        DeterministicPredicted {
            skip_despawn: true,
            ..default()
        },
        Name::from("Ball"),
    ));
    if compound_ball.0 {
        ball.insert((DetBallBodyBundle::dynamic(), Rotation::radians(0.2)));
    } else {
        ball.insert(DetPhysicsBundle::ball());
    }
    if is_state_based {
        ball.insert(PreSpawned::new(BALL_PRESPAWN_HASH));
        if is_server {
            ball.insert((Replicate::to_clients(NetworkTarget::All), CatchUpGated));
        } else if is_client {
            ball.insert(CatchUpGated);
        }
    }
    let ball = ball.id();
    if compound_ball.0 {
        spawn_ball_parts(&mut commands, ball, is_state_based, is_server, is_client);
    }

    let w = WALL_HALF_EXTENT;
    for (start, end) in [
        (Vec2::new(-w, -w), Vec2::new(-w, w)),
        (Vec2::new(-w, w), Vec2::new(w, w)),
        (Vec2::new(w, w), Vec2::new(w, -w)),
        (Vec2::new(w, -w), Vec2::new(-w, -w)),
    ] {
        commands.spawn((
            Collider::segment(start, end),
            ColliderDensity(1.0),
            RigidBody::Static,
            Restitution::new(0.0),
            DetWallMarker,
            Name::from("Wall"),
        ));
    }
}

fn spawn_ball_parts(
    commands: &mut Commands,
    root: Entity,
    state_based: bool,
    is_server: bool,
    is_client: bool,
) {
    fn finish_part(
        entity: &mut EntityCommands,
        part: DetBallPart,
        state_based: bool,
        is_server: bool,
        is_client: bool,
    ) {
        entity.insert(part);
        entity.insert(DeterministicPredicted {
            skip_despawn: true,
            ..default()
        });
        if state_based {
            entity.insert(PreSpawned::new(BALL_PART_PRESPAWN_HASH + part as u64));
            if is_server {
                entity.insert((Replicate::to_clients(NetworkTarget::All), CatchUpGated));
            } else if is_client {
                entity.insert(CatchUpGated);
            }
        }
    }

    let mut core = commands.spawn((
        ChildOf(root),
        Transform::from_xyz(-1.5, -1.0, 0.0).with_rotation(Quat::from_rotation_z(-0.17)),
        Collider::circle(6.0),
        ColliderDensity(0.05),
        Restitution::new(0.8),
        CollisionLayers::default(),
        Name::from("BallCoreCollider"),
    ));
    finish_part(
        &mut core,
        DetBallPart::Core,
        state_based,
        is_server,
        is_client,
    );

    let mut pivot = commands.spawn((
        ChildOf(root),
        Transform::from_xyz(2.0, 1.0, 0.0).with_rotation(Quat::from_rotation_z(0.31)),
        Name::from("BallColliderPivot"),
    ));
    finish_part(
        &mut pivot,
        DetBallPart::Pivot,
        state_based,
        is_server,
        is_client,
    );
    let pivot = pivot.id();

    let mut lobe = commands.spawn((
        ChildOf(pivot),
        Transform::from_xyz(3.0, 0.5, 0.0).with_rotation(Quat::from_rotation_z(0.23)),
        Collider::circle(4.0),
        ColliderDensity(0.04),
        Restitution::new(0.9),
        CollisionLayers::from_bits(0b0010, LayerMask::ALL.0),
        Name::from("BallLobeCollider"),
    ));
    finish_part(
        &mut lobe,
        DetBallPart::Lobe,
        state_based,
        is_server,
        is_client,
    );

    let mut sensor = commands.spawn((
        ChildOf(root),
        Transform::from_xyz(0.5, 1.5, 0.0),
        Collider::circle(10.0),
        Sensor,
        CollisionEventsEnabled,
        CollisionLayers::from_bits(0b0100, LayerMask::ALL.0),
        Name::from("BallSensorCollider"),
    ));
    finish_part(
        &mut sensor,
        DetBallPart::Sensor,
        state_based,
        is_server,
        is_client,
    );
}

/// Convert a `Fire<DetMovement>` trigger's Vec2 value into a velocity
/// acceleration on the player entity.
fn apply_movement(
    trigger: On<Fire<DetMovement>>,
    mut players: Query<(&mut LinearVelocity, Option<&DetPlayerActivationTick>), With<DetPlayerId>>,
    tl: Res<lightyear_core::prelude::LocalTimeline>,
) {
    let Ok((mut velocity, activation_tick)) = players.get_mut(trigger.context) else {
        return;
    };
    let tick = tl.tick();
    if activation_tick.is_some_and(|activation_tick| tick < activation_tick.0) {
        return;
    }
    let input = trigger.value;
    let before = **velocity;
    velocity.x += input.x * MOVE_ACCEL;
    velocity.y += input.y * MOVE_ACCEL;
    **velocity = velocity.clamp_length_max(MAX_VELOCITY);
    if (58..=62).contains(&tick.0) {
        info!(tick = tick.0, context=?trigger.context, input=?input, ?before, after=?**velocity, "apply_movement fire");
    }
}

/// Insert `FrameInterpolate` on every
/// `DeterministicPredicted` entity that has `Position`.
fn add_frame_interpolation(
    trigger: On<Add, DeterministicPredicted>,
    query: Query<(), (With<Position>, Without<FrameInterpolate>)>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(FrameInterpolate);
    }
}

/// Deterministic hash for a client's action entity (so client + server
/// can independently spawn matching PreSpawned pairs).
pub fn action_prespawn_hash(peer: PeerId) -> u64 {
    peer.to_bits()
        .wrapping_mul(6364136223846793005)
        .wrapping_add(0xDEAD_BEEF)
}

/// Re-export BEIBuffer for tests that need to check readiness.
pub type DetBuffer = BEIBuffer<Player>;
