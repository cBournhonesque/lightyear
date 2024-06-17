use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use std::hash::{Hash, Hasher};

use bevy_xpbd_2d::parry::shape::{Ball, SharedShape};
use bevy_xpbd_2d::prelude::*;
use bevy_xpbd_2d::{PhysicsSchedule, PhysicsStepSet};
use leafwing_input_manager::prelude::ActionState;
use lightyear::shared::replication::components::Controlled;
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
        // app.add_systems(FixedPostUpdate, process_collisions);
        app.add_systems(Last, last_log);

        app.add_systems(
            FixedUpdate,
            (/*process_collisions,*/lifetime_despawner).in_set(FixedSet::Main),
        );

        // registry types for reflection
        app.register_type::<Player>();
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

// NB we are not restricting this query to `Controlled` entities on the clients, because we hope to
//    receive PlayerActions for remote players ahead of the server simulating the tick (lag, input delay, etc)
//    in which case we prespawn their bullets on the correct tick, just like we do for our own bullets.
//
//    When spawning here, we add the `PreSpawnedPlayerObject` component, and when the client receives the
//    replication packet from the server, it matches the hashes on its own `PreSpawnedPlayerObject`, allowing it to
//    treat our locally spawned one as the `Predicted` entity (and gives it the Predicted component).
//
//    In cases where the replication packet spawns the bullet BEFORE we get the remote players inputs,
//    (IS THIS POSSIBLE? OR ARE INPUTS ATOMIC ALONGSIDE REPLICATION PACKETS),
//    and then we rollforward over the tick when firing happened, we'd fire another, but would find the
//    dupe with our query to avoid spawning?
//
//    ... hang on.

// not running this in rollback.
//
pub fn shared_player_firing(
    mut q: Query<
        (
            &Position,
            &Rotation,
            &LinearVelocity,
            &ColorComponent,
            &ActionState<PlayerActions>,
            &mut Weapon,
            Has<Controlled>,
            &Player,
        ),
        Or<(With<Predicted>, With<ReplicationTarget>)>,
    >,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    identity: NetworkIdentity,
) {
    if q.is_empty() {
        return;
    }

    let current_tick = tick_manager.tick();
    for (
        player_position,
        player_rotation,
        player_velocity,
        color,
        action,
        mut weapon,
        is_local,
        player,
    ) in q.iter_mut()
    {
        if !action.pressed(&PlayerActions::Fire) {
            continue;
        }
        if (current_tick - weapon.last_fire_tick) <= weapon.cooldown as i16 {
            // cooldown period - can't fire.
            if weapon.last_fire_tick == current_tick {
                // logging because debugging latency edge conditions where
                // inputs arrive on exact frame server replicates to you.
                info!("Can't fire, fired this tick already! {current_tick:?}");
            } else {
                // info!("cooldown. {weapon:?} current_tick = {current_tick:?}");
            }
            continue;
        }
        let prev_last_fire_tick = weapon.last_fire_tick;
        weapon.last_fire_tick = current_tick;

        // bullet spawns just in front of the nose of the ship, in the direction the ship is facing,
        // and inherits the speed of the ship.
        let bullet_spawn_offset = Vec2::Y * (2.0 + (SHIP_LENGTH + BULLET_SIZE) / 2.0);

        let bullet_origin = player_position.0 + player_rotation.rotate(bullet_spawn_offset);
        let bullet_linvel =
            player_rotation.rotate(Vec2::Y * weapon.bullet_speed) + player_velocity.0;

        // create a unique hash for this firing event based on player id and tick number
        // which will match on client and server. unique, because you can't fire twice per tick.
        // (default hasher is unsuitable because it can't distinguish between 2 bullets fired on the
        //  same tick but by different players)
        // TODO different results wasm vs native?
        // let mut hasher = seahash::SeaHasher::new();
        // player.client_id.hash(&mut hasher);
        // weapon.last_fire_tick.hash(&mut hasher);
        // let hash = hasher.finish();
        let hash: u64 = player.client_id.to_bits() ^ (weapon.last_fire_tick.0 as u64);

        let prespawned = PreSpawnedPlayerObject { hash: Some(hash) };

        let bullet_entity = commands
            .spawn((
                BulletBundle::new(bullet_origin, bullet_linvel, color.0, current_tick),
                PhysicsBundle::bullet(),
                prespawned,
            ))
            .id();
        info!(
            "spawned bullet for ActionState, bullet={bullet_entity:?} hash: {hash} ({}, {}). prev last_fire tick: {prev_last_fire_tick:?}",
            weapon.last_fire_tick.0, player.client_id
        );

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
    }
}

// we want clients to predict the despawn due to TTL expiry, so this system runs on both client and server.
// servers despawn without replicating that fact.
pub(crate) fn lifetime_despawner(
    q: Query<(Entity, &Lifetime)>,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    identity: NetworkIdentity,
) {
    for (e, ttl) in q.iter() {
        if (tick_manager.tick() - ttl.origin_tick) > ttl.lifetime {
            // if ttl.origin_tick.wrapping_add(ttl.lifetime) > *tick_manager.tick() {
            if identity.is_server() {
                // info!("Despawning {e:?} without replication");
                // commands.entity(e).despawn_without_replication(); // CRASH ?
                commands.entity(e).remove::<server::Replicate>().despawn();
            } else {
                info!("Despawning:lifetime {e:?}");
                commands.entity(e).despawn_recursive();
            }
        }
    }
}

pub(crate) fn after_physics_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    players: Query<
        (Entity, &Position, &Rotation),
        (Without<BallMarker>, Without<Confirmed>, With<Player>),
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
        (Without<BallMarker>, Without<Confirmed>, With<Player>),
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

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct CollisionPayload;

/// despawn any entities that collide with something and have a
/// CollisionPayload component.
pub(crate) fn process_collisions(
    mut collision_event_reader: EventReader<Collision>,
    payload_q: Query<Entity, With<CollisionPayload>>,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    identity: NetworkIdentity,
) {
    for Collision(contacts) in collision_event_reader.read() {
        // info!("collision {contacts:?}");
        // continue;
        if payload_q.contains(contacts.entity1) {
            if identity.is_server() {
                commands
                    .entity(contacts.entity1)
                    .remove::<server::Replicate>()
                    .despawn();
            } else {
                commands.entity(contacts.entity1).despawn_recursive();
            }
        }
        if payload_q.contains(contacts.entity2) {
            if identity.is_server() {
                commands
                    .entity(contacts.entity2)
                    .remove::<server::Replicate>()
                    .despawn();
            } else {
                commands.entity(contacts.entity2).despawn_recursive();
            }
        }
    }
}
