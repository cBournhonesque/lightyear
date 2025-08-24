use avian2d::prelude::*;
use avian2d::PhysicsPlugins;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use bevy_enhanced_input::action::Action;
use bevy_enhanced_input::prelude::{ActionValue, Completed, Fired, Started};
use core::ops::DerefMut;
use core::time::Duration;
use bevy::platform::collections::HashMap;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client::PeerMetadata;
use lightyear::connection::client_of::ClientOf;
use lightyear::core::tick::TickDuration;
use lightyear::prediction::plugin::PredictionSet;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::prespawn::PreSpawned;
use lightyear::prelude::*;

use crate::protocol::*;

#[cfg(feature = "server")]
use lightyear::prelude::{Room, RoomEvent};

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

        app.add_observer(cycle_replication_mode);
        app.add_observer(cycle_projectile_mode);
        app.add_observer(rotate_player);
        app.add_observer(move_player);
        app.add_observer(shoot_weapon);
        app.add_observer(weapon_cycling);

        app.add_systems(PreUpdate, despawn_after);

        // debug systems
        app.add_systems(FixedLast, fixed_update_log);
        app.add_systems(FixedLast, log_predicted_bot_transform);

        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            (
                predicted_bot_movement,
                simulate_client_projectiles,
                // update_weapon_ring_buffer,
                update_hitscan_visuals,
                update_physics_projectiles,
                update_homing_missiles,
            ),
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

pub(crate) fn rotate_player(
    trigger: Trigger<Fired<MoveCursor>>,
    mut player: Query<(&mut Rotation, &Position), Without<Confirmed>>,
) {
    if let Ok((mut rotation, position)) = player.get_mut(trigger.target()) {
        let angle = Vec2::new(0.0, 1.0).angle_to(trigger.value - position.0);
        // careful to only activate change detection if there was an actual change
        if (angle - rotation.as_radians()).abs() > EPS {
            *rotation = Rotation::from(angle);
        }
    }
}

pub(crate) fn move_player(
    trigger: Trigger<Fired<MovePlayer>>,
    // exclude Confirmed for the case where the all players are interpolated and the
    // user controls the Confirmed entity
    mut player: Query<&mut Position, Without<Confirmed>>
) {
    const PLAYER_MOVE_SPEED: f32 = 10.0;
    if let Ok(mut position) = player.get_mut(trigger.target()) {
        let value = trigger.value;
        if value.x > 0.0 {
            position.x += PLAYER_MOVE_SPEED
        }
        if value.x < 0.0 {
            position.x -= PLAYER_MOVE_SPEED
        }
        if value.y > 0.0 {
            position.y += PLAYER_MOVE_SPEED
        }
        if value.y < 0.0 {
            position.y -= PLAYER_MOVE_SPEED
        }
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
    trigger: Trigger<Completed<CycleWeapon>>,
    mut query: Query<(&mut Weapon, &mut WeaponType)>,
) {
    if let Ok((mut weapon, mut weapon_type)) = query.get_mut(trigger.target()) {
        let new_weapon_type = weapon_type.next();
        *weapon_type = new_weapon_type;
        weapon.weapon_type = new_weapon_type;

        // Update weapon properties based on type
        match new_weapon_type {
            WeaponType::Hitscan => weapon.fire_rate = 5.0,
            WeaponType::LinearProjectile => weapon.fire_rate = 2.0,
            WeaponType::Shotgun => weapon.fire_rate = 1.0,
            WeaponType::PhysicsProjectile => weapon.fire_rate = 1.5,
            WeaponType::HomingMissile => weapon.fire_rate = 0.5,
        }

        info!("Switched to weapon: {}", new_weapon_type.name());
    }
}

/// Main weapon shooting system that handles all weapon types
pub(crate) fn shoot_weapon(
    trigger: Trigger<Completed<Shoot>>,
    mut commands: Commands,
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    time: Res<Time>,
    tick_duration: Res<TickDuration>,
    query: SpatialQuery,
    mut player_query: Query<
        (
            &PlayerId,
            &Transform,
            &ColorComponent,
            &mut Weapon,
            &WeaponType,
            Option<&ControlledBy>,
        ),
        (With<PlayerMarker>, Without<Confirmed>)
    >,
    client: Query<(&ProjectileReplicationMode, &GameReplicationMode)>,
) {
    let tick = timeline.tick();
    let tick_duration = tick_duration.0;

    if let Ok((id, transform, color, mut weapon, weapon_type, controlled_by)) =
        player_query.get_mut(trigger.target())
    {
        let is_server = controlled_by.is_some();
        let (projectile_mode, replication_mode) = controlled_by.map_or_else(|| client.single().unwrap(),|c| client.get(c.owner).unwrap());

        // Check fire rate
        if let Some(last_fire) = weapon.last_fire_tick {
            let ticks_since_last_fire = tick.0.saturating_sub(last_fire.0);
            let time_since_last_fire = Duration::from_secs_f64(ticks_since_last_fire as f64 / 64.0);
            let min_fire_interval = Duration::from_secs_f32(1.0 / weapon.fire_rate);

            if time_since_last_fire < min_fire_interval {
                return; // Too soon to fire again
            }
        }

        weapon.last_fire_tick = Some(tick);

        // Handle replication mode before shooting
        match projectile_mode {
            ProjectileReplicationMode::FullEntity => {
                shoot_with_full_entity_replication(
                    &mut commands,
                    &timeline,
                    transform,
                    id,
                    color,
                    controlled_by,
                    is_server,
                    weapon_type,
                    replication_mode
                );
            }
            ProjectileReplicationMode::DirectionOnly => {
                shoot_with_direction_only_replication(
                    &mut commands,
                    &timeline,
                    transform,
                    id,
                    color,
                    controlled_by,
                    is_server,
                    replication_mode,
                    weapon_type,
                );
            }
            ProjectileReplicationMode::RingBuffer => {
                shoot_with_ring_buffer_replication(
                    &mut weapon,
                    &timeline,
                    transform,
                    id,
                    weapon_type,
                );
            }
        }
    }
}

/// Full entity replication: spawn a replicated entity for the projectile
fn shoot_with_full_entity_replication(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
    weapon_type: &WeaponType,
    replication_mode: &GameReplicationMode
) {
    match weapon_type {
        WeaponType::Hitscan => {
            shoot_hitscan(
                commands,
                timeline,
                transform,
                id,
                color,
                controlled_by,
                replication_mode,
                false,
            );
        }
        WeaponType::LinearProjectile => {
            shoot_linear_projectile(
                commands,
                timeline,
                transform,
                id,
                color,
                controlled_by,
                replication_mode,
                is_server,
            );
        }
        WeaponType::Shotgun => {
            // shoot_shotgun(
            //     commands,
            //     timeline,
            //     transform,
            //     id,
            //     color,
            //     controlled_by,
            //     replication_mode,
            //     is_server,
            // );
        }
        WeaponType::PhysicsProjectile => {
            // shoot_physics_projectile(
            //     commands,
            //     timeline,
            //     transform,
            //     id,
            //     color,
            //     controlled_by,
            //     replication_mode,
            //     is_server,
            // );
        }
        WeaponType::HomingMissile => {
            // let target = find_nearest_target(transform);
            // shoot_homing_missile(
            //     commands,
            //     timeline,
            //     transform,
            //     id,
            //     color,
            //     controlled_by,
            //     is_server,
            //     target,
            // );
        }
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
    replication_mode: &GameReplicationMode,
    weapon_type: &WeaponType,
) {
    let direction = transform.up().as_vec3().truncate();
    let position = transform.translation.truncate();
    let speed = match weapon_type {
        WeaponType::Hitscan => 1000.0, // Instant
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

    // TODO: instead of replicating an entity; we can just send a one-off message?
    //  but how to do prediction?
    // TODO: with entity:
    //  - prediction: client predicted the SpawnInfo entity and spawned children projectile entities
    //     if we were mispredicting and the client didn't shoot, then we have to despawn all the predicted projectiles
    //  - interpolation: ideally the client interpolates by adjusting the position to account for the exact fire tick

    if is_server {
        match replication_mode {
            GameReplicationMode::AllPredicted => {
                commands.spawn((
                    spawn_info,
                    *color,
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::All),
                    controlled_by.unwrap().clone(),
                    PreSpawned::default(),
                    Name::new("ProjectileSpawn"),
                ));
            }
            GameReplicationMode::ClientPredictedNoComp => {
                commands.spawn((
                    spawn_info,
                    *color,
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    PreSpawned::default(),
                    controlled_by.unwrap().clone(),
                    Name::new("ProjectileSpawn"),
                ));
            }
            GameReplicationMode::ClientPredictedLagComp => {
                commands.spawn((
                    spawn_info,
                    *color,
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::All),
                    controlled_by.unwrap().clone(),
                    PreSpawned::default(),
                    Name::new("ProjectileSpawn"),
                ));
            }
            GameReplicationMode::ClientSideHitDetection => {
                commands.spawn((
                    spawn_info,
                    *color,
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::All),
                    controlled_by.unwrap().clone(),
                    PreSpawned::default(),
                    Name::new("ProjectileSpawn"),
                ));
            }
            GameReplicationMode::AllInterpolated => {}
            GameReplicationMode::OnlyInputsReplicated => {}
        }

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

#[derive(Component)]
pub struct ClientHitDetection;


// fn hitscan_hit_detection(
//     mut commands: Commands,
//     players: Query<&Position, With<PlayerMarker>>,
//     mut query: Query<(&mut Transform, &mut LinearVelocity, &mut HitscanVisual,  ClientHitDetection)>,
// ) {
//     for (mut transform, mut linear_velocity, mut hitscan_visual, mut client_hit_detection) in query.iter_mut() {

// }


fn shoot_hitscan(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    replication_mode: &GameReplicationMode,
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
        match replication_mode {
            GameReplicationMode::AllPredicted => {
                // clients predict other clients using their inputs
                // TODO: how does it work for shots fired by others?
            },
            GameReplicationMode::ClientPredictedNoComp => {
                commands.spawn((
                    visual_bundle,
                    // no need to replicate to the shooting player since they are predicting their shot
                    Replicate::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                ));
                // TODO: do hit detection without lag comp

            },
            GameReplicationMode::ClientPredictedLagComp  => {
                commands.spawn((
                    visual_bundle,
                    // no need to replicate to the shooting player since they are predicting their shot
                    Replicate::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                ));
                // TODO: do hit detection with lag comp
            }
            GameReplicationMode::ClientSideHitDetection => {
                commands.spawn((
                    visual_bundle,
                    // no need to replicate to the shooting player since they are predicting their shot
                    Replicate::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    ClientHitDetection,
                ));
                // TODO: client detects hits for the bullets they fire and then send message to the server
            }
            GameReplicationMode::AllInterpolated => {
                commands.spawn((
                    visual_bundle,
                    Replicate::to_clients(NetworkTarget::All)
                ));
            }
            GameReplicationMode::OnlyInputsReplicated => {}
        }
    } else {
        // Visuals are purely client-side

        match replication_mode {
            GameReplicationMode::AllPredicted => {
                // should we predict other clients shooting?
                commands.spawn(visual_bundle);
            },
            GameReplicationMode::ClientPredictedNoComp | GameReplicationMode::ClientPredictedLagComp  => {
                commands.spawn(visual_bundle);
            }
            GameReplicationMode::ClientSideHitDetection => {
                commands.spawn(visual_bundle);
                // do hit detection
            }
            GameReplicationMode::AllInterpolated => {
                // we don't spawn the visuals, it will be replicated to us
            }
            GameReplicationMode::OnlyInputsReplicated => {
                // do hit detection
                commands.spawn(visual_bundle);
            }
        }
    }
}

fn shoot_linear_projectile(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    replication_mode: &GameReplicationMode,
    is_server: bool,
) {
    let bullet_transform = transform.clone();
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
        match replication_mode {
            GameReplicationMode::AllPredicted => {}
            GameReplicationMode::ClientPredictedNoComp => {
                commands.spawn((
                    bullet_bundle,
                    PreSpawned::default(),
                    DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    controlled_by.unwrap().clone(),
                ));
                // TODO: hit detection
            }
            GameReplicationMode::ClientPredictedLagComp => {
                commands.spawn((
                    bullet_bundle,
                    PreSpawned::default(),
                    DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    controlled_by.unwrap().clone(),
                ));
                // TODO: hit detection
            }
            GameReplicationMode::ClientSideHitDetection => {
                commands.spawn((
                    bullet_bundle,
                    PreSpawned::default(),
                    DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    controlled_by.unwrap().clone(),
                ));
            }
            GameReplicationMode::AllInterpolated => {
                commands.spawn((
                    bullet_bundle,
                    DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
                    Replicate::to_clients(NetworkTarget::All),
                    InterpolationTarget::to_clients(NetworkTarget::All),
                    controlled_by.unwrap().clone(),
                ));
            }
            GameReplicationMode::OnlyInputsReplicated => {}
        }
    } else {
        match replication_mode {
            GameReplicationMode::AllPredicted => {
                // should we predict other clients shooting?
                commands.spawn((bullet_bundle, PreSpawned::default()));
            },
            GameReplicationMode::ClientPredictedNoComp | GameReplicationMode::ClientPredictedLagComp  => {
                commands.spawn((bullet_bundle, PreSpawned::default()));
            }
            GameReplicationMode::ClientSideHitDetection => {
                commands.spawn((bullet_bundle, PreSpawned::default()));
                // do hit detection
            }
            GameReplicationMode::AllInterpolated => {
                // we don't spawn anything, it will be replicated to us
            }
            GameReplicationMode::OnlyInputsReplicated => {
                commands.spawn((bullet_bundle, DeterministicPredicted));
            }
        }
    }
}

fn shoot_shotgun(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    replication_mode: &GameReplicationMode,
    is_server: bool,
) {
    let pellet_count = 8;
    let spread_angle = 0.3; // 30 degrees spread

    for i in 0..pellet_count {
        let angle_offset =
            (i as f32 - (pellet_count - 1) as f32 / 2.0) * spread_angle / (pellet_count - 1) as f32;
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
    replication_mode: &GameReplicationMode,
    is_server: bool,
) {
    let bullet_transform = transform.clone();
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
    let missile_transform = transform.clone();
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
    bot_query: &Query<
        (Entity, &Transform),
        (
            Or<(With<PredictedBot>, With<InterpolatedBot>)>,
            Without<PlayerMarker>,
        ),
    >,
) -> Option<Entity> {
    let player_pos = transform.translation.truncate();

    bot_query
        .iter()
        .min_by(|(_, a_transform), (_, b_transform)| {
            let a_dist = a_transform
                .translation
                .truncate()
                .distance_squared(player_pos);
            let b_dist = b_transform
                .translation
                .truncate()
                .distance_squared(player_pos);
            a_dist
                .partial_cmp(&b_dist)
                .unwrap_or(std::cmp::Ordering::Equal)
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
    target_query: Query<
        &Transform,
        (
            Or<(With<PredictedBot>, With<InterpolatedBot>)>,
            Without<HomingMissile>,
        ),
    >,
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
    mut query: Query<(
        Entity,
        &mut Transform,
        &mut LinearVelocity,
        &ClientProjectile,
    )>,
) {
    let current_tick = timeline.tick();

    for (entity, mut transform, velocity, projectile) in query.iter_mut() {
        let ticks_elapsed = current_tick.0.saturating_sub(projectile.spawn_tick.0);
        let time_elapsed = ticks_elapsed as f32 / 64.0; // Assuming 64 Hz fixed timestep

        // Update position based on physics simulation
        let expected_position =
            projectile.start_position + projectile.direction * projectile.speed * time_elapsed;
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
    mut query: Query<
        (
            &mut Weapon,
            &PlayerId,
            &ColorComponent,
            Option<&ControlledBy>,
        ),
        With<PlayerMarker>,
    >,

) {
    let current_tick = timeline.tick();

    for (mut weapon, player_id, color, controlled_by) in query.iter_mut() {

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
                spawn_projectile_from_buffer(
                    &mut commands,
                    projectile_info,
                    player_id,
                    color,
                    controlled_by,
                    is_server,
                );
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
pub struct Rooms {
    pub rooms: HashMap<GameReplicationMode, Entity>,
}

impl Default for Rooms {
    fn default() -> Self {
        Self { rooms: HashMap::default() }
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

pub fn cycle_replication_mode(
    trigger: Trigger<Completed<CycleReplicationMode>>,
    mut client: Query<&mut GameReplicationMode>,
) {
    if let Ok(mut replication_mode) = client.get_mut(trigger.target()) {
        *replication_mode = replication_mode.next();
        info!("Done cycling replication mode to {:?}. Entity: {:?}", replication_mode, trigger.target());
    }
}

pub fn cycle_projectile_mode(
    trigger: Trigger<Completed<CycleProjectileMode>>,
    mut client: Query<&mut ProjectileReplicationMode>,
) {
    if let Ok(mut projectile_mode) = client.get_mut(trigger.target()) {
        *projectile_mode = projectile_mode.next();
        info!("Done cycling projectile mode to {:?}. Entity: {:?}", projectile_mode, trigger.target());
    }
}


pub fn player_bundle(client_id: PeerId) -> impl Bundle {
    let y = (client_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    let color = color_from_id(client_id);
    (
        // the context needs to be inserted on the server, and will be replicated to the client
        PlayerContext,
        Score(0),
        PlayerId(client_id),
        RigidBody::Kinematic,
        Transform::from_xyz(0.0, y, 0.0),
        ColorComponent(color),
        PlayerMarker,
        Weapon::default(),
        WeaponType::default(),
        Name::new("Player"),
    )
}