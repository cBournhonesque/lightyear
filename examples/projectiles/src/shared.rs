use avian2d::prelude::*;
use avian2d::PhysicsPlugins;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client_of::ClientOf;
use lightyear::prediction::plugin::PredictionSet;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::prespawn::PreSpawned;
use lightyear::prelude::*;

use crate::protocol::*;

#[cfg(feature = "server")]
use lightyear::visibility::room::{Room, RoomEvent};

const EPS: f32 = 0.0001;
pub const BOT_RADIUS: f32 = 15.0;
pub(crate) const BOT_MOVE_SPEED: f32 = 1.0;
const BULLET_MOVE_SPEED: f32 = 300.0;
const MAP_LIMIT: f32 = 2000.0;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.register_type::<PlayerId>();

        // Add lightyear's room system for interest management
        #[cfg(feature = "server")]
        app.add_plugins(lightyear::visibility::room::RoomPlugin);

        app.add_systems(Startup, setup_replication_rooms);
        app.init_resource::<ReplicationRooms>();

        app.add_systems(PreUpdate, despawn_after);

        // debug systems
        app.add_systems(FixedLast, fixed_update_log);
        app.add_systems(FixedLast, log_predicted_bot_transform);

        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            (
                predicted_bot_movement,
                player_movement,
                weapon_cycling,
                replication_mode_cycling,
                room_cycling,
                manage_room_membership,
                shoot_weapon,
                simulate_client_projectiles,
                update_weapon_ring_buffer,
                update_hitscan_visuals,
                update_physics_projectiles,
                update_homing_missiles,
            ).chain(),
        );
        // both client and server need physics
        // (the client also needs the physics plugin to be able to compute predicted bullet hits)
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable Sync as it is handled by lightyear_avian
                .disable::<SyncPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_player_movement(
    mut position: Mut<Position>,
    mut rotation: Mut<Rotation>,
    action: &ActionState<PlayerActions>,
) {
    const PLAYER_MOVE_SPEED: f32 = 10.0;
    let Some(cursor_data) = action.dual_axis_data(&PlayerActions::MoveCursor) else {
        return;
    };
    let angle = Vec2::new(0.0, 1.0).angle_to(cursor_data.pair - position.0);
    // careful to only activate change detection if there was an actual change
    if (angle - rotation.as_radians()).abs() > EPS {
        *rotation = Rotation::from(angle);
    }
    // TODO: look_at should work
    // transform.look_at(Vec3::new(mouse_position.x, mouse_position.y, 0.0), Vec3::Y);
    if action.pressed(&PlayerActions::Up) {
        position.y += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        position.y -= PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        position.x += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        position.x -= PLAYER_MOVE_SPEED;
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut player_query: Query<
        (
            &mut Position,
            &mut Rotation,
            &ActionState<PlayerActions>,
            &PlayerId,
        ),
        (Or<(With<Predicted>, With<Replicate>)>, With<PlayerMarker>),
    >,
) {
    for (position, rotation, action_state, player_id) in player_query.iter_mut() {
        debug!(tick = ?timeline.tick(), action = ?action_state.dual_axis_data(&PlayerActions::MoveCursor), "Data in Movement (FixedUpdate)");
        shared_player_movement(position, rotation, action_state);
    }
}

fn predicted_bot_movement(
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut query: Query<&mut Position, (With<PredictedBot>, Or<(With<Predicted>, With<Replicating>)>)>,
) {
    let tick = timeline.tick();
    query.iter_mut().for_each(|mut position| {
        let direction = if (tick.0 / 200) % 2 == 0 { 1.0 } else { -1.0 };
        position.x += BOT_MOVE_SPEED * direction;
    });
}

fn log_predicted_bot_transform(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    query: Query<
        (&Position, &Transform),
        (With<PredictedBot>, Or<(With<Predicted>, With<Replicating>)>),
    >,
) {
    let (timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
    query.iter().for_each(|(pos, transform)| {
        debug!(?tick, ?pos, ?transform, "PredictedBot FixedLast");
    })
}

pub(crate) fn fixed_update_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    player: Query<(Entity, &Transform), (With<PlayerMarker>, With<PlayerId>, Without<Confirmed>)>,
    predicted_bullet: Query<
        (Entity, &Transform, Option<&PredictionHistory<Transform>>),
        (With<BulletMarker>, Without<Confirmed>),
    >,
) {
    let (timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
    for (entity, transform) in player.iter() {
        debug!(
            ?tick,
            ?entity,
            pos = ?transform.translation.truncate(),
            "Player after fixed update"
        );
    }
    for (entity, transform, history) in predicted_bullet.iter() {
        debug!(
            ?tick,
            ?entity,
            pos = ?transform.translation.truncate(),
            ?history,
            "Bullet after fixed update"
        );
    }
}

/// Handle weapon cycling input
pub(crate) fn weapon_cycling(
    mut query: Query<
        (&mut Weapon, &mut WeaponType, &ActionState<PlayerActions>),
        (Or<(With<Predicted>, With<Replicate>)>, With<PlayerMarker>),
    >,
) {
    for (mut weapon, mut weapon_type, action) in query.iter_mut() {
        if action.just_pressed(&PlayerActions::CycleWeapon) {
            let new_weapon_type = weapon_type.next();
            *weapon_type = new_weapon_type;
            weapon.weapon_type = new_weapon_type;

            // Update weapon properties based on type
            match new_weapon_type {
                WeaponType::Hitscan => weapon.fire_rate = 5.0,
                WeaponType::HitscanSlowVisuals => weapon.fire_rate = 3.0,
                WeaponType::LinearProjectile => weapon.fire_rate = 2.0,
                WeaponType::Shotgun => weapon.fire_rate = 1.0,
                WeaponType::PhysicsProjectile => weapon.fire_rate = 1.5,
                WeaponType::HomingMissile => weapon.fire_rate = 0.5,
            }

            info!("Switched to weapon: {}", new_weapon_type.name());
        }
    }
}

/// Handle replication mode cycling input
pub(crate) fn replication_mode_cycling(
    mut query: Query<
        (&mut Weapon, &ActionState<PlayerActions>),
        (Or<(With<Predicted>, With<Replicate>)>, With<PlayerMarker>),
    >,
) {
    for (mut weapon, action) in query.iter_mut() {
        if action.just_pressed(&PlayerActions::CycleReplicationMode) {
            let new_mode = weapon.projectile_replication_mode.next();
            weapon.projectile_replication_mode = new_mode;

            info!("Switched to replication mode: {}", new_mode.name());
        }
    }
}

// Room cycling function is now implemented later in the file with proper room management

/// Main weapon shooting system that handles all weapon types
pub(crate) fn shoot_weapon(
    mut commands: Commands,
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    time: Res<Time>,
    query: Query<&SpatialQuery, With<Server>>,
    mut player_query: Query<
        (
            &PlayerId,
            &Transform,
            &ColorComponent,
            &mut ActionState<PlayerActions>,
            &mut Weapon,
            &WeaponType,
            Option<&ControlledBy>,
        ),
        (Or<(With<Predicted>, With<Replicate>)>, With<PlayerMarker>),
    >,
    // Query for potential homing targets
    bot_query: Query<(Entity, &Transform), (Or<(With<PredictedBot>, With<InterpolatedBot>)>, Without<PlayerMarker>)>,
) {
    let tick = timeline.tick();
    let tick_duration = Duration::from_secs_f64(1.0 / 64.0); // Assuming 64 Hz fixed timestep

    for (id, transform, color, action, mut weapon, weapon_type, controlled_by) in player_query.iter_mut() {
        let is_server = controlled_by.is_some();

        if action.just_pressed(&PlayerActions::Shoot) {
            // Check fire rate
            if let Some(last_fire) = weapon.last_fire_tick {
                let ticks_since_last_fire = tick.0.saturating_sub(last_fire.0);
                let time_since_last_fire = Duration::from_secs_f64(ticks_since_last_fire as f64 / 64.0);
                let min_fire_interval = Duration::from_secs_f64(1.0 / weapon.fire_rate);

                if time_since_last_fire < min_fire_interval {
                    continue; // Too soon to fire again
                }
            }

            weapon.last_fire_tick = Some(tick);

            // Handle replication mode before shooting
            match weapon.projectile_replication_mode {
                ProjectileReplicationMode::FullEntity => {
                    shoot_with_full_entity_replication(&mut commands, &timeline, transform, id, color, controlled_by, is_server, weapon_type, &bot_query);
                },
                ProjectileReplicationMode::DirectionOnly => {
                    shoot_with_direction_only_replication(&mut commands, &timeline, transform, id, color, controlled_by, is_server, weapon_type);
                },
                ProjectileReplicationMode::RingBuffer => {
                    shoot_with_ring_buffer_replication(&mut weapon, &timeline, transform, id, weapon_type);
                },
            }
        }
    }
}

/// Full entity replication - current behavior
fn shoot_with_full_entity_replication(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
    weapon_type: &WeaponType,
    bot_query: &Query<(Entity, &Transform), (Or<(With<PredictedBot>, With<InterpolatedBot>)>, Without<PlayerMarker>)>,
) {
    match weapon_type {
        WeaponType::Hitscan => {
            shoot_hitscan(commands, timeline, transform, id, color, controlled_by, false);
        },
        WeaponType::HitscanSlowVisuals => {
            shoot_hitscan(commands, timeline, transform, id, color, controlled_by, true);
        },
        WeaponType::LinearProjectile => {
            shoot_linear_projectile(commands, timeline, transform, id, color, controlled_by, is_server);
        },
        WeaponType::Shotgun => {
            shoot_shotgun(commands, timeline, transform, id, color, controlled_by, is_server);
        },
        WeaponType::PhysicsProjectile => {
            shoot_physics_projectile(commands, timeline, transform, id, color, controlled_by, is_server);
        },
        WeaponType::HomingMissile => {
            let target = find_nearest_target(transform, bot_query);
            shoot_homing_missile(commands, timeline, transform, id, color, controlled_by, is_server, target);
        },
    }
}

/// Direction-only replication - only replicate spawn parameters
fn shoot_with_direction_only_replication(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
    weapon_type: &WeaponType,
) {
    let direction = transform.up().as_vec3().truncate();
    let position = transform.translation.truncate();
    let speed = match weapon_type {
        WeaponType::Hitscan | WeaponType::HitscanSlowVisuals => 1000.0, // Instant
        WeaponType::LinearProjectile => BULLET_MOVE_SPEED,
        WeaponType::Shotgun => BULLET_MOVE_SPEED * 0.8,
        WeaponType::PhysicsProjectile => BULLET_MOVE_SPEED * 0.6,
        WeaponType::HomingMissile => BULLET_MOVE_SPEED * 0.4,
    };

    let spawn_info = ProjectileSpawn {
        spawn_tick: timeline.tick(),
        position,
        direction,
        speed,
        weapon_type: *weapon_type,
        player_id: id.0,
    };

    if is_server {
        #[cfg(feature = "server")]
        commands.spawn((
            spawn_info,
            *color,
            Replicate::to_clients(NetworkTarget::All),
            controlled_by.unwrap().clone(),
            Name::new("ProjectileSpawn"),
        ));
    } else {
        // Client creates the projectile immediately for prediction
        create_client_projectile(commands, &spawn_info, color);
    }
}

/// Ring buffer replication - store projectiles in weapon component
fn shoot_with_ring_buffer_replication(
    weapon: &mut Weapon,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    weapon_type: &WeaponType,
) {
    let direction = transform.up().as_vec3().truncate();
    let position = transform.translation.truncate();

    let projectile_info = ProjectileSpawnInfo {
        spawn_tick: timeline.tick(),
        position,
        direction,
        weapon_type: *weapon_type,
    };

    // Add to ring buffer
    weapon.projectile_buffer.push(projectile_info);
    if weapon.projectile_buffer.len() > weapon.buffer_capacity {
        weapon.projectile_buffer.remove(0); // Remove oldest
    }
}

fn shoot_hitscan(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    slow_visuals: bool,
) {
    let is_server = controlled_by.is_some();
    let direction = transform.up().as_vec3().truncate();
    let start = transform.translation.truncate();
    let end = start + direction * 1000.0; // Long hitscan range

    // Create visual effect
    let visual_lifetime = if slow_visuals { 0.5 } else { 0.1 };
    let visual_bundle = (
        HitscanVisual {
            start,
            end,
            lifetime: 0.0,
            max_lifetime: visual_lifetime,
        },
        *color,
        *id,
        Name::new("HitscanVisual"),
    );

    if is_server {
        #[cfg(feature = "server")]
        commands.spawn((
            visual_bundle,
            Replicate::to_clients(NetworkTarget::All),
            controlled_by.unwrap().clone(),
        ));
    } else {
        commands.spawn(visual_bundle);
    }

    // TODO: Implement actual hit detection for hitscan
}

fn shoot_linear_projectile(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
) {
    let mut bullet_transform = transform.clone();
    let bullet_bundle = (
        bullet_transform,
        LinearVelocity(bullet_transform.up().as_vec3().truncate() * BULLET_MOVE_SPEED),
        RigidBody::Kinematic,
        *id,
        *color,
        BulletMarker,
        Name::new("LinearProjectile"),
    );

    if is_server {
        #[cfg(feature = "server")]
        commands.spawn((
            bullet_bundle,
            PreSpawned::default(),
            DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
            controlled_by.unwrap().clone(),
        ));
    } else {
        commands.spawn((bullet_bundle, PreSpawned::default()));
    }
}

fn shoot_shotgun(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
) {
    let pellet_count = 8;
    let spread_angle = 0.3; // 30 degrees spread

    for i in 0..pellet_count {
        let angle_offset = (i as f32 - (pellet_count - 1) as f32 / 2.0) * spread_angle / (pellet_count - 1) as f32;
        let mut pellet_transform = transform.clone();
        pellet_transform.rotate_z(angle_offset);

        let pellet_bundle = (
            pellet_transform,
            LinearVelocity(pellet_transform.up().as_vec3().truncate() * BULLET_MOVE_SPEED * 0.8),
            RigidBody::Kinematic,
            *id,
            *color,
            BulletMarker,
            ShotgunPellet {
                pellet_index: i,
                spread_angle: angle_offset,
            },
            Name::new("ShotgunPellet"),
        );

        let salt = i as u64;

        if is_server {
            #[cfg(feature = "server")]
            commands.spawn((
                pellet_bundle,
                PreSpawned::default_with_salt(salt),
                DespawnAfter(Timer::new(Duration::from_secs(2), TimerMode::Once)),
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                controlled_by.unwrap().clone(),
            ));
        } else {
            commands.spawn((pellet_bundle, PreSpawned::default_with_salt(salt)));
        }
    }
}

fn shoot_physics_projectile(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
) {
    let mut bullet_transform = transform.clone();
    let bullet_bundle = (
        bullet_transform,
        LinearVelocity(bullet_transform.up().as_vec3().truncate() * BULLET_MOVE_SPEED * 0.6),
        RigidBody::Dynamic, // Use dynamic for physics interactions
        Collider::circle(BULLET_SIZE),
        Restitution::new(0.8), // Bouncy
        *id,
        *color,
        BulletMarker,
        PhysicsProjectile {
            bounce_count: 0,
            max_bounces: 3,
            deceleration: 50.0,
        },
        Name::new("PhysicsProjectile"),
    );

    if is_server {
        #[cfg(feature = "server")]
        commands.spawn((
            bullet_bundle,
            PreSpawned::default(),
            DespawnAfter(Timer::new(Duration::from_secs(5), TimerMode::Once)),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
            controlled_by.unwrap().clone(),
        ));
    } else {
        commands.spawn((bullet_bundle, PreSpawned::default()));
    }
}

fn shoot_homing_missile(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
    target: Option<Entity>,
) {
    let mut missile_transform = transform.clone();
    let missile_bundle = (
        missile_transform,
        LinearVelocity(missile_transform.up().as_vec3().truncate() * BULLET_MOVE_SPEED * 0.4),
        RigidBody::Kinematic,
        *id,
        *color,
        BulletMarker,
        HomingMissile {
            target_entity: target,
            turn_speed: 2.0,
            acceleration: 100.0,
        },
        Name::new("HomingMissile"),
    );

    if is_server {
        #[cfg(feature = "server")]
        commands.spawn((
            missile_bundle,
            PreSpawned::default(),
            DespawnAfter(Timer::new(Duration::from_secs(8), TimerMode::Once)),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
            controlled_by.unwrap().clone(),
        ));
    } else {
        commands.spawn((missile_bundle, PreSpawned::default()));
    }
}

fn find_nearest_target(
    transform: &Transform,
    bot_query: &Query<(Entity, &Transform), (Or<(With<PredictedBot>, With<InterpolatedBot>)>, Without<PlayerMarker>)>,
) -> Option<Entity> {
    let player_pos = transform.translation.truncate();

    bot_query
        .iter()
        .min_by(|(_, a_transform), (_, b_transform)| {
            let a_dist = a_transform.translation.truncate().distance_squared(player_pos);
            let b_dist = b_transform.translation.truncate().distance_squared(player_pos);
            a_dist.partial_cmp(&b_dist).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(entity, _)| entity)
}

/// Update hitscan visual effects
pub(crate) fn update_hitscan_visuals(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(Entity, &mut HitscanVisual)>,
) {
    for (entity, mut visual) in query.iter_mut() {
        visual.lifetime += time.delta_secs();
        if visual.lifetime >= visual.max_lifetime {
            commands.entity(entity).despawn();
        }
    }
}

/// Update physics projectiles (apply deceleration)
pub(crate) fn update_physics_projectiles(
    time: Res<Time>,
    mut query: Query<(&mut LinearVelocity, &PhysicsProjectile)>,
) {
    for (mut velocity, physics) in query.iter_mut() {
        let current_speed = velocity.length();
        if current_speed > 0.0 {
            let deceleration = physics.deceleration * time.delta_secs();
            let new_speed = (current_speed - deceleration).max(0.0);
            velocity.0 = velocity.normalize() * new_speed;
        }
    }
}

/// Update homing missiles
pub(crate) fn update_homing_missiles(
    time: Res<Time>,
    mut missile_query: Query<(&mut Transform, &mut LinearVelocity, &HomingMissile)>,
    target_query: Query<&Transform, (Or<(With<PredictedBot>, With<InterpolatedBot>)>, Without<HomingMissile>)>,
) {
    for (mut missile_transform, mut velocity, homing) in missile_query.iter_mut() {
        if let Some(target_entity) = homing.target_entity {
            if let Ok(target_transform) = target_query.get(target_entity) {
                let missile_pos = missile_transform.translation.truncate();
                let target_pos = target_transform.translation.truncate();
                let to_target = (target_pos - missile_pos).normalize();

                let current_dir = velocity.normalize();
                let turn_factor = homing.turn_speed * time.delta_secs();
                let new_dir = (current_dir + to_target * turn_factor).normalize();

                let current_speed = velocity.length();
                let new_speed = current_speed + homing.acceleration * time.delta_secs();

                velocity.0 = new_dir * new_speed;

                // Update missile rotation to face direction
                let angle = Vec2::new(0.0, 1.0).angle_to(new_dir);
                missile_transform.rotation = Quat::from_rotation_z(angle);
            }
        }
    }
}

/// Create a client-side projectile for direction-only replication
fn create_client_projectile(
    commands: &mut Commands,
    spawn_info: &ProjectileSpawn,
    color: &ColorComponent,
) {
    let mut transform = Transform::from_translation(spawn_info.position.extend(0.0));
    let angle = Vec2::new(0.0, 1.0).angle_to(spawn_info.direction);
    transform.rotation = Quat::from_rotation_z(angle);

    let velocity = spawn_info.direction * spawn_info.speed;

    commands.spawn((
        transform,
        LinearVelocity(velocity),
        RigidBody::Kinematic,
        PlayerId(spawn_info.player_id),
        *color,
        BulletMarker,
        ClientProjectile {
            start_position: spawn_info.position,
            direction: spawn_info.direction,
            speed: spawn_info.speed,
            spawn_tick: spawn_info.spawn_tick,
            weapon_type: spawn_info.weapon_type,
        },
        Name::new("ClientProjectile"),
    ));
}

/// System to simulate client projectiles for direction-only replication
pub(crate) fn simulate_client_projectiles(
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut Transform, &mut LinearVelocity, &ClientProjectile)>,
) {
    let current_tick = timeline.tick();

    for (entity, mut transform, mut velocity, projectile) in query.iter_mut() {
        let ticks_elapsed = current_tick.0.saturating_sub(projectile.spawn_tick.0);
        let time_elapsed = ticks_elapsed as f32 / 64.0; // Assuming 64 Hz fixed timestep

        // Update position based on physics simulation
        let expected_position = projectile.start_position + projectile.direction * projectile.speed * time_elapsed;
        transform.translation = expected_position.extend(0.0);

        // Despawn after 3 seconds
        if time_elapsed > 3.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// System to process ring buffer projectiles and spawn them
pub(crate) fn update_weapon_ring_buffer(
    mut commands: Commands,
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut query: Query<(&mut Weapon, &PlayerId, &ColorComponent, Option<&ControlledBy>), With<PlayerMarker>>,
) {
    let current_tick = timeline.tick();

    for (mut weapon, player_id, color, controlled_by) in query.iter_mut() {
        if weapon.projectile_replication_mode != ProjectileReplicationMode::RingBuffer {
            continue;
        }

        // Process projectiles that should be spawned based on their tick
        let mut projectiles_to_spawn = Vec::new();

        for (i, projectile_info) in weapon.projectile_buffer.iter().enumerate() {
            let ticks_since_spawn = current_tick.0.saturating_sub(projectile_info.spawn_tick.0);

            // Spawn if it's the right time (within the last few ticks to avoid missing)
            if ticks_since_spawn <= 2 {
                projectiles_to_spawn.push(i);
            }
        }

        let is_server = controlled_by.is_some();

        for &index in &projectiles_to_spawn {
            if let Some(projectile_info) = weapon.projectile_buffer.get(index) {
                spawn_projectile_from_buffer(&mut commands, projectile_info, player_id, color, controlled_by, is_server);
            }
        }

        // Clean up old projectiles from buffer
        weapon.projectile_buffer.retain(|p| {
            let ticks_since_spawn = current_tick.0.saturating_sub(p.spawn_tick.0);
            ticks_since_spawn < 64 * 5 // Keep for 5 seconds
        });
    }
}

fn spawn_projectile_from_buffer(
    commands: &mut Commands,
    projectile_info: &ProjectileSpawnInfo,
    player_id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
) {
    let mut transform = Transform::from_translation(projectile_info.position.extend(0.0));
    let angle = Vec2::new(0.0, 1.0).angle_to(projectile_info.direction);
    transform.rotation = Quat::from_rotation_z(angle);

    let speed = match projectile_info.weapon_type {
        WeaponType::LinearProjectile => BULLET_MOVE_SPEED,
        WeaponType::Shotgun => BULLET_MOVE_SPEED * 0.8,
        WeaponType::PhysicsProjectile => BULLET_MOVE_SPEED * 0.6,
        WeaponType::HomingMissile => BULLET_MOVE_SPEED * 0.4,
        _ => BULLET_MOVE_SPEED, // Default for hitscan weapons (though they shouldn't use ring buffer)
    };

    let velocity = projectile_info.direction * speed;

    let bullet_bundle = (
        transform,
        LinearVelocity(velocity),
        RigidBody::Kinematic,
        *player_id,
        *color,
        BulletMarker,
        Name::new("RingBufferProjectile"),
    );

    if is_server {
        #[cfg(feature = "server")]
        commands.spawn((
            bullet_bundle,
            DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(player_id.0)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(player_id.0)),
            controlled_by.unwrap().clone(),
        ));
    } else {
        commands.spawn(bullet_bundle);
    }
}

#[derive(Component)]
struct DespawnAfter(pub Timer);

/// Resource to track room entities for each replication mode
#[derive(Resource)]
pub struct ReplicationRooms {
    pub rooms: Vec<Entity>, // One room per GameReplicationMode
}

impl Default for ReplicationRooms {
    fn default() -> Self {
        Self {
            rooms: Vec::new(),
        }
    }
}

/// Setup rooms for each replication mode on startup
fn setup_replication_rooms(
    mut commands: Commands,
    mut rooms: ResMut<ReplicationRooms>,
) {
    // Create one room for each GameReplicationMode (6 rooms total)
    for i in 0..6 {
        #[cfg(feature = "server")]
        {
            let room_entity = commands.spawn((
                Room::default(),
                Name::new(format!("ReplicationRoom_{}", i)),
            )).id();
            rooms.rooms.push(room_entity);
        }

        #[cfg(not(feature = "server"))]
        {
            // On client, just create placeholder entities to keep indices consistent
            let room_entity = commands.spawn(Name::new(format!("ReplicationRoom_{}", i))).id();
            rooms.rooms.push(room_entity);
        }
    }
    info!("Created {} replication rooms", rooms.rooms.len());
}

/// Handle room cycling input and update room membership
pub(crate) fn room_cycling(
    mut query: Query<
        (&mut PlayerRoom, &ActionState<PlayerActions>, Entity),
        (Or<(With<Predicted>, With<Replicate>)>, With<PlayerMarker>),
    >,
    rooms: Res<ReplicationRooms>,
    mut commands: Commands,
) {
    for (mut player_room, action, player_entity) in query.iter_mut() {
        if action.just_pressed(&PlayerActions::CycleRoom) {
            let current_mode = GameReplicationMode::from_room_id(player_room.room_id);
            let new_mode = current_mode.next();
            let new_room_id = new_mode.room_id();

            #[cfg(feature = "server")]
            {
                // Remove from old room
                if let Some(old_room) = rooms.rooms.get(player_room.room_id as usize) {
                    commands.trigger_targets(RoomEvent::RemoveSender(player_entity), *old_room);
                }

                // Add to new room
                if let Some(new_room) = rooms.rooms.get(new_room_id as usize) {
                    commands.trigger_targets(RoomEvent::AddSender(player_entity), *new_room);
                }
            }

            player_room.room_id = new_room_id;
            info!("Player switched to room: {} ({})", new_room_id, new_mode.name());
        }
    }
}

/// Manage room membership for entities and players
pub(crate) fn manage_room_membership(
    // Track players joining rooms for the first time
    new_players: Query<(Entity, &PlayerRoom), (Added<PlayerRoom>, With<PlayerMarker>)>,
    // Track entities that should be in rooms
    replicated_entities: Query<Entity, (With<Replicate>, Without<PlayerMarker>)>,
    rooms: Res<ReplicationRooms>,
    mut commands: Commands,
) {
    #[cfg(feature = "server")]
    {
        // Add new players to their rooms
        for (player_entity, player_room) in new_players.iter() {
            if let Some(room_entity) = rooms.rooms.get(player_room.room_id as usize) {
                commands.trigger_targets(RoomEvent::AddSender(player_entity), *room_entity);
                info!("Added player {:?} to room {}", player_entity, player_room.room_id);
            }
        }

        // Add all replicated entities to all rooms (for now - in a real implementation,
        // you might want different logic based on the replication mode)
        for entity in replicated_entities.iter() {
            for room_entity in &rooms.rooms {
                commands.trigger_targets(RoomEvent::AddEntity(entity), *room_entity);
            }
        }
    }
}

/// Despawn entities after their timer has finished
fn despawn_after(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut DespawnAfter)>,
) {
    for (entity, mut despawn_after) in query.iter_mut() {
        despawn_after.0.tick(time.delta());
        if despawn_after.0.finished() {
            commands.entity(entity).despawn();
        }
    }
}
