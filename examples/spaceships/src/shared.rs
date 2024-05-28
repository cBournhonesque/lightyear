use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;

use bevy_xpbd_2d::parry::shape::{Ball, SharedShape};
use bevy_xpbd_2d::prelude::*;
use bevy_xpbd_2d::{PhysicsSchedule, PhysicsStepSet};
use leafwing_input_manager::prelude::ActionState;
use tracing::Level;

use lightyear::prelude::client::*;
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
use lightyear::shared::ping::diagnostics::PingDiagnosticsPlugin;
use lightyear::transport::io::IoDiagnosticsPlugin;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::{protocol::*, renderer};
pub(crate) const MAX_VELOCITY: f32 = 200.0;
pub(crate) const WALL_SIZE: f32 = 350.0;

use crate::renderer::SpaceshipsRendererPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum FixedSet {
    // main fixed update systems (handle inputs)
    Main,
    // apply physics steps
    Physics,
}

#[derive(Clone)]
pub struct SharedPlugin {
    pub(crate) show_confirmed: bool,
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_plugins(SpaceshipsRendererPlugin);
        }
        // bundles
        app.add_systems(Startup, init);

        // physics
        app.add_plugins(PhysicsPlugins::new(FixedUpdate))
            .insert_resource(Time::new_with(Physics::fixed_once_hz(FIXED_TIMESTEP_HZ)))
            .insert_resource(Gravity(Vec2::ZERO));
        app.configure_sets(
            FixedUpdate,
            (
                // make sure that any physics simulation happens after the Main SystemSet
                // (where we apply user's actions)
                (
                    PhysicsSet::Prepare,
                    PhysicsSet::StepSimulation,
                    PhysicsSet::Sync,
                )
                    .in_set(FixedSet::Physics),
                (FixedSet::Main, FixedSet::Physics).chain(),
            ),
        );
        // add a log at the start of the physics schedule
        app.add_systems(PhysicsSchedule, log.in_set(PhysicsStepSet::BroadPhase));

        app.add_systems(FixedPostUpdate, after_physics_log);
        app.add_systems(Last, last_log);

        // registry types for reflection
        app.register_type::<PlayerId>();
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
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

#[derive(QueryData)]
#[query_data(mutable, derive(Debug))]
pub struct ApplyInputsQuery {
    pub ex_force: &'static mut ExternalForce,
    pub ang_vel: &'static mut AngularVelocity,
    pub rot: &'static Rotation,
    pub action: &'static ActionState<PlayerActions>,
}

pub fn shared_movement_behaviour(aiq: ApplyInputsQueryItem) {
    const THRUSTER_POWER: f32 = 32000.;
    const ROTATIONAL_SPEED: f32 = 4.0;
    let ApplyInputsQueryItem {
        mut ex_force,
        mut ang_vel,
        rot,
        action,
    } = aiq;

    // info!("pressed: {:?}", action.get_pressed());

    if action.pressed(&PlayerActions::Up) {
        ex_force
            .apply_force(rot.rotate(Vec2::Y * THRUSTER_POWER))
            .with_persistence(false);
    }
    let desired_ang_vel = if action.pressed(&PlayerActions::Left) {
        ROTATIONAL_SPEED
    } else if action.pressed(&PlayerActions::Right) {
        -ROTATIONAL_SPEED
    } else {
        0.0
    };
    if ang_vel.0 != desired_ang_vel {
        ang_vel.0 = desired_ang_vel;
    }
}

// NB we are not restricting this query to `Controlled` entities on the clients, because it's possible one can
//    receive PlayerActions for remote players ahead of the server simulating the tick (lag, input delay, etc)
//    in which case we prespawn their bullets on the correct tick, just like we do for our own bullets.
pub fn shared_player_firing(
    mut q: Query<
        (
            &Position,
            &Rotation,
            &LinearVelocity,
            &ColorComponent,
            &ActionState<PlayerActions>,
            &mut Weapon,
        ),
        (
            With<PlayerId>,
            Or<(With<Predicted>, With<ReplicationTarget>)>,
        ),
    >,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    identity: NetworkIdentity,
) {
    for (pos, rot, vel, color, action, mut weapon) in q.iter_mut() {
        if !action.pressed(&PlayerActions::Fire) {
            continue;
        }
        if (weapon.last_fire_tick + Tick(weapon.cooldown)) > tick_manager.tick() {
            // cooldown period - can't fire.
            continue;
        }
        weapon.last_fire_tick = tick_manager.tick();

        let bullet_entity = spawn_bullet(&mut commands, pos, rot, vel, color.0);
        if identity.is_server() {
            let replicate = server::Replicate {
                sync: server::SyncTarget {
                    prediction: NetworkTarget::All,
                    ..Default::default()
                },
                // make sure that all entities that are predicted are part of the same replication group
                group: REPLICATION_GROUP,
                ..default()
            };
            commands.entity(bullet_entity).insert(replicate);
        }
        // info!("spawned bullet {bullet_entity:?}");
    }
}

// On clients, we need to add PreSpawnedPlayerObject
// On server, we need to add Replicate
pub fn spawn_bullet(
    commands: &mut Commands,
    player_position: &Position,
    player_rotation: &Rotation,
    player_velocity: &LinearVelocity,
    color: Color,
) -> Entity {
    let bullet_spawn_offset = Vec2::Y * (SHIP_LENGTH / 2.0 + 1.0);
    let bullet_speed = 500.0;

    let bullet_origin = player_position.0 + player_rotation.rotate(bullet_spawn_offset);
    let bullet_linvel = player_rotation.rotate(Vec2::Y * bullet_speed) + player_velocity.0;

    commands
        .spawn((
            BulletMarker,
            Position(bullet_origin),
            LinearVelocity(bullet_linvel),
            PhysicsBundle::bullet(),
            ColorComponent(color),
            PreSpawnedPlayerObject::default(),
        ))
        .id()
}

pub(crate) fn after_physics_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    players: Query<
        (Entity, &Position, &Rotation),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = rollback.map_or(tick_manager.tick(), |r| {
        tick_manager.tick_or_rollback_tick(r.as_ref())
    });
    for (entity, position, rotation) in players.iter() {
        debug!(
            ?tick,
            ?entity,
            ?position,
            rotation = ?rotation.as_degrees(),
            "Player after physics update"
        );
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball after physics update");
    }
}

pub(crate) fn last_log(
    tick_manager: Res<TickManager>,
    players: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Option<&Correction<Position>>,
            Option<&Correction<Rotation>>,
        ),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = tick_manager.tick();
    for (entity, position, rotation, correction, rotation_correction) in players.iter() {
        debug!(?tick, ?entity, ?position, ?correction, "Player LAST update");
        debug!(
            ?tick,
            ?entity,
            rotation = ?rotation.as_degrees(),
            ?rotation_correction,
            "Player LAST update"
        );
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball LAST update");
    }
}

pub(crate) fn log() {
    debug!("run physics schedule!");
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
                external_force: ExternalForce::default(),
            },
            wall: Wall { start, end },
            name: Name::new("Wall"),
        }
    }
}
