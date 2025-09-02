use avian2d::PhysicsPlugins;
use avian2d::prelude::*;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use bevy_enhanced_input::action::Action;
use bevy_enhanced_input::prelude::{ActionValue, Completed, Fired, Started};
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client::PeerMetadata;
use lightyear::connection::client_of::ClientOf;
use lightyear::core::tick::TickDuration;
use lightyear::prediction::plugin::PredictionSet;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::prespawn::PreSpawned;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::LagCompensationSpatialQuery;

use crate::protocol::*;

#[cfg(feature = "server")]
use lightyear::prelude::{Room, RoomEvent};

const EPS: f32 = 0.0001;
const BULLET_MOVE_SPEED: f32 = 300.0;
const MAP_LIMIT: f32 = 2000.0;
const HITSCAN_COLLISION_DISTANCE_CHECK: f32 = 2000.0;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.register_type::<PlayerId>();

        app.add_observer(rotate_player);
        app.add_observer(move_player);
        app.add_observer(shoot_weapon);
        app.add_observer(weapon_cycling);

        // projectile spawning
        app.add_observer(handle_projectile_spawn);

        // hit detection
        app.add_observer(hitscan_hit_detection);
        app.add_systems(FixedUpdate, bullet_hit_detection);

        app.add_systems(PreUpdate, despawn_after);

        // debug systems
        app.add_systems(FixedLast, fixed_update_log);
        app.add_systems(Last, last_log);

        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            (
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
    mut player: Query<(&mut Rotation, &Position), (Without<Confirmed>, Without<Bot>)>,
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
    // Confirmed inputs don't get applied on the client! (for the AllInterpolated case)
    mut player: Query<&mut Position, Without<Confirmed>>,
    is_bot: Query<(), With<Bot>>,
) {
    const PLAYER_MOVE_SPEED: f32 = 2.0;
    if let Ok(mut position) = player.get_mut(trigger.target()) {
        if is_bot.get(trigger.target()).is_err() {
            trace!(
                ?position,
                "Moving player {:?} by {:?}",
                trigger.target(),
                trigger.value
            );
        }
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

pub(crate) fn fixed_update_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    player: Query<
        (Entity, &Position),
        (
            With<PlayerMarker>,
            With<PlayerId>,
            Without<Confirmed>,
            Without<Bot>,
        ),
    >,
    // predicted_bullet: Query<
    //     (Entity, &Position, Option<&PredictionHistory<Position>>),
    //     (With<BulletMarker>, Without<Confirmed>),
    // >,
) {
    let (timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
    for (entity, pos) in player.iter() {
        debug!(?tick, ?entity, ?pos, "Player after fixed update");
    }
    // for (entity, transform, history) in predicted_bullet.iter() {
    //     debug!(
    //         ?tick,
    //         ?entity,
    //         pos = ?transform.translation.truncate(),
    //         ?history,
    //         "Bullet after fixed update"
    //     );
    // }
}

pub(crate) fn last_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    player: Query<
        (Entity, &Position, &Transform),
        (
            With<PlayerMarker>,
            With<PlayerId>,
            Without<Confirmed>,
            Without<Bot>,
        ),
    >,
    // predicted_bullet: Query<
    //     (Entity, &Position, Option<&PredictionHistory<Position>>),
    //     (With<BulletMarker>, Without<Confirmed>),
    // >,
) {
    let (timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
    for (entity, pos, transform) in player.iter() {
        debug!(
            ?tick,
            ?entity,
            ?pos,
            transform = ?transform.translation,
            "Player after last"
        );
    }
    // for (entity, transform, history) in predicted_bullet.iter() {
    //     debug!(
    //         ?tick,
    //         ?entity,
    //         pos = ?transform.translation.truncate(),
    //         ?history,
    //         "Bullet after fixed update"
    //     );
    // }
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
        With<PlayerMarker>,
    >,
    global: Single<(&ProjectileReplicationMode, &GameReplicationMode), With<ClientContext>>,
) {
    let tick = timeline.tick();
    let tick_duration = tick_duration.0;
    let shooter = trigger.target();
    let (projectile_mode, replication_mode) = global.into_inner();

    if let Ok((id, transform, color, mut weapon, weapon_type, controlled_by)) =
        player_query.get_mut(trigger.target())
    {
        let is_server = controlled_by.is_some();
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

        info!(
            ?weapon_type,
            ?projectile_mode,
            ?tick,
            "Player {:?} shooting",
            shooter
        );
        // Handle replication mode before shooting
        match (weapon_type, projectile_mode) {
            //
            (_, ProjectileReplicationMode::FullEntity) => {
                shoot_with_full_entity_replication(
                    &mut commands,
                    &timeline,
                    transform,
                    id,
                    shooter,
                    color,
                    controlled_by,
                    is_server,
                    weapon_type,
                    replication_mode,
                );
            }
            (_, ProjectileReplicationMode::DirectionOnly) => {
                shoot_with_direction_only_replication(
                    &mut commands,
                    &timeline,
                    transform,
                    id,
                    shooter,
                    color,
                    controlled_by,
                    is_server,
                    replication_mode,
                    weapon_type,
                );
            }
            (_, ProjectileReplicationMode::RingBuffer) => {
                shoot_with_ring_buffer_replication(
                    &mut weapon,
                    &timeline,
                    transform,
                    id,
                    shooter,
                    weapon_type,
                );
            }
        }
    }
}

/// Full entity replication: spawn a replicated entity for the projectile
/// The entity keeps getting replicated from server to clients
fn shoot_with_full_entity_replication(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    shooter: Entity,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    is_server: bool,
    weapon_type: &WeaponType,
    replication_mode: &GameReplicationMode,
) {
    match weapon_type {
        WeaponType::Hitscan => {
            shoot_hitscan(
                commands,
                timeline,
                transform,
                id,
                shooter,
                color,
                controlled_by,
                replication_mode,
            );
        }
        WeaponType::LinearProjectile => {
            shoot_linear_projectile(
                commands,
                timeline,
                transform,
                id,
                shooter,
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
    shooter: Entity,
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

    // TODO: for hitscan, maybe we can just do similar to FullEntity replication? We add HitscanVisual
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
        #[cfg(feature = "server")]
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
        create_client_projectile(commands, &spawn_info, color, shooter);
    }
}

/// Ring buffer replication - store projectiles in weapon component
fn shoot_with_ring_buffer_replication(
    weapon: &mut Weapon,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    shooter: Entity,
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

pub(crate) fn hitscan_hit_detection(
    trigger: Trigger<OnAdd, HitscanVisual>,
    commands: Commands,
    server: Query<Entity, With<Server>>,
    timeline: Query<&LocalTimeline, Without<ClientOf>>,
    mode: Query<&GameReplicationMode, With<ClientContext>>,
    mut spatial_set: ParamSet<(LagCompensationSpatialQuery, SpatialQuery)>,
    bullet: Query<(&HitscanVisual, &BulletMarker, &PlayerId)>,
    target_query: Query<(), (With<PlayerMarker>, Without<Confirmed>)>,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    client_query: Query<&InterpolationDelay, With<ClientOf>>,
    mut player_query: Query<(&mut Score, &PlayerId, Option<&ControlledBy>), With<PlayerMarker>>,
) {
    let Ok(timeline) = timeline.single() else {
        info!("no unique timeline");
        return;
    };
    let Ok(mode) = mode.single() else {
        info!("no unique mode");
        return;
    };
    let Ok((hitscan, bullet_marker, id)) = bullet.get(trigger.target()) else {
        return;
    };

    let shooter = bullet_marker.shooter;
    let direction = (hitscan.end - hitscan.start).normalize();

    let tick = timeline.tick();
    let is_server = server.single().is_ok();

    // check if we should be running hit detection on the server or client
    if is_server {
        if mode == &GameReplicationMode::ClientSideHitDetection
            || mode == &GameReplicationMode::OnlyInputsReplicated
        {
            return;
        }
    } else {
        if mode != &GameReplicationMode::ClientSideHitDetection
            && mode != &GameReplicationMode::OnlyInputsReplicated
        {
            return;
        }
        // TODO: ignore bullets that were fired by other clients
    }
    info!(?hitscan, "Hit detection for hitscan");

    match mode {
        GameReplicationMode::ClientPredictedLagComp => {
            let Ok(Some(controlled_by)) = player_query
                .get(shooter)
                .map(|(_, _, controlled_by)| controlled_by)
            else {
                error!("Could not retrieve controlled_by for client {id:?}");
                return;
            };
            let Ok(delay) = client_query.get(controlled_by.owner) else {
                error!("Could not retrieve InterpolationDelay for client {id:?}");
                return;
            };
            let query = spatial_set.p0();
            if let Some(hit_data) = query.cast_ray_predicate(
                // the delay is sent in every input message; the latest InterpolationDelay received
                // is stored on the client entity
                *delay,
                hitscan.start,
                Dir2::new_unchecked(direction),
                HITSCAN_COLLISION_DISTANCE_CHECK,
                false,
                // we stop on the first time the predicate is true, i.e. if we shoot a Player entity
                // this is important to not hit the lag compensation colliders
                &|entity| target_query.get(entity).is_ok(),
                &mut SpatialQueryFilter::from_excluded_entities([shooter]),
            ) {
                let target = hit_data.entity;
                info!(?tick, ?hit_data, ?shooter, ?target, "Hitscan hit detected");
                // if there is a hit, increment the score
                if is_server && let Ok((mut score, _, _)) = player_query.get_mut(shooter) {
                    info!("Increment score");
                    score.0 += 1;
                }
            }
        }
        _ => {
            let query = spatial_set.p1();
            if let Some(hit_data) = query.cast_ray_predicate(
                hitscan.start,
                Dir2::new_unchecked(direction),
                HITSCAN_COLLISION_DISTANCE_CHECK,
                false,
                &mut SpatialQueryFilter::from_excluded_entities([shooter]),
                // we stop on the first time the predicate is true, i.e. if we shoot a Player entity
                // this is important to not hit the lag compensation colliders
                &|entity| target_query.get(entity).is_ok(),
            ) {
                let target = hit_data.entity;
                info!(
                    ?mode,
                    ?tick,
                    ?hit_data,
                    ?shooter,
                    ?target,
                    "Hitscan hit detected"
                );
                // if there is a hit, increment the score
                if is_server && let Ok((mut score, _, _)) = player_query.get_mut(shooter) {
                    info!("Increment score");
                    score.0 += 1;
                }
            }
        }
    }
}

pub(crate) fn bullet_hit_detection(
    commands: Commands,
    server: Query<Entity, With<Server>>,
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mode: Single<&GameReplicationMode, With<ClientContext>>,
    mut spatial_set: ParamSet<(LagCompensationSpatialQuery, SpatialQuery)>,
    bullet: Query<(&Position, &LinearVelocity, &BulletMarker, &PlayerId)>,
    target_query: Query<(), (With<PlayerMarker>, Without<Confirmed>)>,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    client_query: Query<&InterpolationDelay, With<ClientOf>>,
    mut player_query: Query<(&mut Score, &PlayerId, Option<&ControlledBy>), With<PlayerMarker>>,
) {
    let tick = timeline.tick();
    let is_server = server.single().is_ok();
    let mode = mode.into_inner();

    // check if we should be running hit detection on the server or client
    if is_server {
        if mode == &GameReplicationMode::ClientSideHitDetection
            || mode == &GameReplicationMode::OnlyInputsReplicated
        {
            return;
        }
    } else {
        if mode != &GameReplicationMode::ClientSideHitDetection
            && mode != &GameReplicationMode::OnlyInputsReplicated
        {
            return;
        }
        // TODO: ignore bullets that were fired by other clients
    }
    bullet
        .iter()
        .for_each(|(position, velocity, bullet_marker, id)| {
            let shooter = bullet_marker.shooter;
            let direction = velocity.0.normalize();
            let start = position.0;
            let max_distance = velocity.0.length();

            match mode {
                GameReplicationMode::ClientPredictedLagComp => {
                    let Ok(Some(controlled)) = player_query
                        .get(shooter)
                        .map(|(_, _, controlled_by)| controlled_by)
                    else {
                        error!("Could not retrieve controlled_by for client {id:?}");
                        return;
                    };
                    let Ok(delay) = client_query.get(controlled.owner) else {
                        error!("Could not retrieve InterpolationDelay for client {id:?}");
                        return;
                    };
                    let query = spatial_set.p0();
                    if let Some(hit_data) = query.cast_ray(
                        // the delay is sent in every input message; the latest InterpolationDelay received
                        // is stored on the client entity
                        *delay,
                        start,
                        Dir2::new_unchecked(direction),
                        max_distance,
                        false,
                        &mut SpatialQueryFilter::default(),
                    ) {
                        let target = hit_data.entity;
                        info!(?tick, ?hit_data, ?shooter, ?target, "Hitscan hit detected");
                        // if there is a hit, increment the score
                        player_query
                            .iter_mut()
                            .find(|(_, player_id, _)| player_id.0 == id.0)
                            .map(|(mut score, _, _)| {
                                score.0 += 1;
                            });
                    }
                }
                _ => {
                    let query = spatial_set.p1();
                    if let Some(hit_data) = query.cast_ray_predicate(
                        start,
                        Dir2::new_unchecked(direction),
                        max_distance,
                        false,
                        &SpatialQueryFilter::default(),
                        &|entity| target_query.get(entity).is_ok(),
                    ) {
                        let target = hit_data.entity;
                        info!(
                            ?mode,
                            ?tick,
                            ?hit_data,
                            ?shooter,
                            ?target,
                            "Hitscan hit detected"
                        );
                        // if there is a hit, increment the score
                        player_query
                            .iter_mut()
                            .find(|(_, player_id, _)| player_id.0 == id.0)
                            .map(|(mut score, _, _)| {
                                score.0 += 1;
                            });
                    }
                }
            }
        });
}

fn shoot_hitscan(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    shooter: Entity,
    color: &ColorComponent,
    controlled_by: Option<&ControlledBy>,
    replication_mode: &GameReplicationMode,
) {
    let is_server = controlled_by.is_some();
    let direction = transform.up().as_vec3().truncate();
    let start = transform.translation.truncate();
    let end = start + direction * 1000.0; // Long hitscan range

    // For Hitscan, we directly spawn an entity that represents the 'bullet'
    let spawn_bundle = (
        HitscanVisual {
            start,
            end,
            lifetime: 0.0,
            max_lifetime: 0.1,
        },
        BulletMarker { shooter },
        *color,
        *id,
        Name::new("HitscanProjectileSpawn"),
    );
    let collision_layers = CollisionLayers::from(replication_mode.room_layer());

    if is_server {
        #[cfg(feature = "server")]
        match replication_mode {
            GameReplicationMode::AllPredicted => {
                // clients predict other clients using their inputs. We still shoot the visual on the server
                // because the hit detection is done on server-side
                //
                // clients don't predict other clients shooting, though (unless they have enough input delay?)
                // I guess no need to replicate to clients since the entity dies so quickly
                commands.spawn((
                    spawn_bundle,
                    // no need to replicate to the shooting player since they are predicting their shot
                    // and it's very short-lived
                    Replicate::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    PredictionTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    controlled_by.unwrap().clone(),
                    PreSpawned::default(),
                    collision_layers,
                ));
                // TODO: how does it work for shots fired by others?
            }
            GameReplicationMode::ClientPredictedNoComp => {
                commands.spawn((
                    spawn_bundle,
                    // no need to replicate to the shooting player since they are predicting their shot
                    // ans it's very short-lived
                    Replicate::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    controlled_by.unwrap().clone(),
                    collision_layers,
                ));
            }
            GameReplicationMode::ClientPredictedLagComp => {
                commands.spawn((
                    spawn_bundle,
                    // no need to replicate to the shooting player since they are predicting their shot
                    // and it's very short-lived
                    Replicate::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    controlled_by.unwrap().clone(),
                    collision_layers,
                ));
            }
            GameReplicationMode::ClientSideHitDetection => {
                commands.spawn((
                    spawn_bundle,
                    // no need to replicate to the shooting player since they are predicting their shot
                    Replicate::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                    controlled_by.unwrap().clone(),
                    PreSpawned::default(),
                    ClientHitDetection,
                    collision_layers,
                ));
            }
            GameReplicationMode::AllInterpolated => {
                commands.spawn((
                    spawn_bundle,
                    Replicate::to_clients(NetworkTarget::All),
                    InterpolationTarget::to_clients(NetworkTarget::All),
                    controlled_by.unwrap().clone(),
                    collision_layers,
                ));
            }
            GameReplicationMode::OnlyInputsReplicated => {}
        }
    } else {
        // Visuals are purely client-side
        match replication_mode {
            GameReplicationMode::AllPredicted => {
                // should we predict other clients shooting? I guess yes?
                //  this observer will also trigger for remove clients
                commands.spawn((spawn_bundle, PreSpawned::default()));
            }
            GameReplicationMode::ClientPredictedNoComp
            | GameReplicationMode::ClientPredictedLagComp => {
                commands.spawn((spawn_bundle, PreSpawned::default()));
            }
            GameReplicationMode::ClientSideHitDetection => {
                commands.spawn((spawn_bundle, PreSpawned::default()));
                // do hit detection
            }
            GameReplicationMode::AllInterpolated => {
                // we don't spawn anything, it will be replicated to us
            }
            GameReplicationMode::OnlyInputsReplicated => {
                // do hit detection
                commands.spawn((spawn_bundle, DeterministicPredicted));
            }
        }
    }
}

fn shoot_linear_projectile(
    commands: &mut Commands,
    timeline: &LocalTimeline,
    transform: &Transform,
    id: &PlayerId,
    shooter: Entity,
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
        BulletMarker { shooter },
        Name::new("LinearProjectile"),
    );

    if is_server {
        #[cfg(feature = "server")]
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
            }
            GameReplicationMode::ClientPredictedNoComp
            | GameReplicationMode::ClientPredictedLagComp => {
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
    shooter: Entity,
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
            BulletMarker { shooter },
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
    shooter: Entity,
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
        BulletMarker { shooter },
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
    shooter: Entity,
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
        BulletMarker { shooter },
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
                .unwrap_or(core::cmp::Ordering::Equal)
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
    shooter: Entity,
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
        BulletMarker { shooter },
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
            Entity,
            &mut Weapon,
            &PlayerId,
            &ColorComponent,
            Option<&ControlledBy>,
        ),
        With<PlayerMarker>,
    >,
) {
    let current_tick = timeline.tick();

    for (shooter, mut weapon, player_id, color, controlled_by) in query.iter_mut() {
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
                    shooter,
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
    shooter: Entity,
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
        BulletMarker { shooter },
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
#[derive(Resource, Debug)]
pub struct Rooms {
    pub rooms: HashMap<GameReplicationMode, Entity>,
}

impl Default for Rooms {
    fn default() -> Self {
        Self {
            rooms: HashMap::default(),
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

/// Handle ProjectileSpawn by spawning child entities for projectiles
pub(crate) fn handle_projectile_spawn(
    trigger: Trigger<OnAdd, ProjectileSpawn>,
    mut commands: Commands,
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    tick_duration: Res<TickDuration>,
    spawn_query: Query<(
        &ProjectileSpawn,
        &ColorComponent,
        &BulletMarker,
        Has<Predicted>,
        Has<Interpolated>,
    )>,
) {
    let Ok((spawn_info, color, bullet_marker, is_predicted, is_interpolated)) =
        spawn_query.get(trigger.target())
    else {
        return;
    };

    let current_tick = timeline.tick();
    let shooter = bullet_marker.shooter;

    match spawn_info.weapon_type {
        WeaponType::Hitscan => {
            // TODO: spawn the hitscan at the right time on the Interpolation Timeline!
            // Create hitscan visual child entity
            spawn_hitscan_visual(
                &mut commands,
                spawn_info,
                color,
                shooter,
                current_tick,
                is_predicted,
                is_interpolated,
            );
        }
        WeaponType::LinearProjectile => {
            // Create linear projectile child entity
            spawn_linear_projectile_child(
                &mut commands,
                spawn_info,
                color,
                shooter,
                current_tick,
                tick_duration.0,
                is_predicted,
                is_interpolated,
            );
        }
        WeaponType::Shotgun => {
            // Create shotgun pellets
            spawn_shotgun_pellets(
                &mut commands,
                spawn_info,
                color,
                shooter,
                current_tick,
                tick_duration.0,
                is_predicted,
                is_interpolated,
            );
        }
        WeaponType::PhysicsProjectile => {
            // Create physics projectile child entity
            spawn_physics_projectile_child(
                &mut commands,
                spawn_info,
                color,
                shooter,
                current_tick,
                tick_duration.0,
                is_predicted,
                is_interpolated,
            );
        }
        WeaponType::HomingMissile => {
            // Create homing missile child entity
            spawn_homing_missile_child(
                &mut commands,
                spawn_info,
                color,
                shooter,
                current_tick,
                tick_duration.0,
                is_predicted,
                is_interpolated,
            );
        }
    }
}

/// Spawn hitscan visual as child entity
fn spawn_hitscan_visual(
    commands: &mut Commands,
    spawn_info: &ProjectileSpawn,
    color: &ColorComponent,
    shooter: Entity,
    current_tick: Tick,
    is_predicted: bool,
    is_interpolated: bool,
) {
    let start = spawn_info.position;
    let end = start + spawn_info.direction * 1000.0;

    let visual_bundle = (
        HitscanVisual {
            start,
            end,
            lifetime: 0.0,
            max_lifetime: 0.1,
        },
        BulletMarker { shooter },
        *color,
        PlayerId(spawn_info.player_id),
        Name::new("HitscanVisual"),
    );
    info!("Spawning hitscan visual: {:?}", visual_bundle);

    if is_predicted {
        commands.spawn((visual_bundle, PreSpawned::default()));
    } else if is_interpolated {
        commands.spawn(visual_bundle);
    } else {
        commands.spawn(visual_bundle);
    }
}

/// Spawn linear projectile as child entity
fn spawn_linear_projectile_child(
    commands: &mut Commands,
    spawn_info: &ProjectileSpawn,
    color: &ColorComponent,
    shooter: Entity,
    current_tick: Tick,
    tick_duration: Duration,
    is_predicted: bool,
    is_interpolated: bool,
) {
    let mut position = spawn_info.position;
    let mut transform = Transform::from_translation(position.extend(0.0));

    // For interpolation, adjust spawn position to account for delay
    if is_interpolated {
        let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
        let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
        position += spawn_info.direction * spawn_info.speed * time_elapsed;
        transform.translation = position.extend(0.0);
    }

    let angle = Vec2::new(0.0, 1.0).angle_to(spawn_info.direction);
    transform.rotation = Quat::from_rotation_z(angle);

    let bullet_bundle = (
        transform,
        LinearVelocity(spawn_info.direction * spawn_info.speed),
        RigidBody::Kinematic,
        PlayerId(spawn_info.player_id),
        *color,
        BulletMarker { shooter },
        DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
        Name::new("LinearProjectile"),
    );

    if is_predicted {
        commands.spawn((bullet_bundle, PreSpawned::default()));
    } else {
        commands.spawn(bullet_bundle);
    }
}

/// Spawn shotgun pellets as child entities
fn spawn_shotgun_pellets(
    commands: &mut Commands,
    spawn_info: &ProjectileSpawn,
    color: &ColorComponent,
    shooter: Entity,
    current_tick: Tick,
    tick_duration: Duration,
    is_predicted: bool,
    is_interpolated: bool,
) {
    let pellet_count = 8;
    let spread_angle = 0.3; // 30 degrees spread

    for i in 0..pellet_count {
        let angle_offset =
            (i as f32 - (pellet_count - 1) as f32 / 2.0) * spread_angle / (pellet_count - 1) as f32;

        let rotation = Quat::from_rotation_z(angle_offset);
        let pellet_direction = rotation * spawn_info.direction.extend(0.0);
        let pellet_direction = pellet_direction.truncate();

        let mut position = spawn_info.position;
        let mut transform = Transform::from_translation(position.extend(0.0));

        // For interpolation, adjust spawn position to account for delay
        if is_interpolated {
            let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
            let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
            position += pellet_direction * spawn_info.speed * 0.8 * time_elapsed;
            transform.translation = position.extend(0.0);
        }

        let angle = Vec2::new(0.0, 1.0).angle_to(pellet_direction);
        transform.rotation = Quat::from_rotation_z(angle);

        let pellet_bundle = (
            transform,
            LinearVelocity(pellet_direction * spawn_info.speed * 0.8),
            RigidBody::Kinematic,
            PlayerId(spawn_info.player_id),
            *color,
            BulletMarker { shooter },
            ShotgunPellet {
                pellet_index: i,
                spread_angle: angle_offset,
            },
            DespawnAfter(Timer::new(Duration::from_secs(2), TimerMode::Once)),
            Name::new("ShotgunPellet"),
        );

        let salt = i as u64;
        if is_predicted {
            commands.spawn((pellet_bundle, PreSpawned::default_with_salt(salt)));
        } else {
            commands.spawn(pellet_bundle);
        }
    }
}

/// Spawn physics projectile as child entity
fn spawn_physics_projectile_child(
    commands: &mut Commands,
    spawn_info: &ProjectileSpawn,
    color: &ColorComponent,
    shooter: Entity,
    current_tick: Tick,
    tick_duration: Duration,
    is_predicted: bool,
    is_interpolated: bool,
) {
    let mut position = spawn_info.position;
    let mut transform = Transform::from_translation(position.extend(0.0));

    // For interpolation, adjust spawn position to account for delay
    if is_interpolated {
        let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
        let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
        position += spawn_info.direction * spawn_info.speed * 0.6 * time_elapsed;
        transform.translation = position.extend(0.0);
    }

    let angle = Vec2::new(0.0, 1.0).angle_to(spawn_info.direction);
    transform.rotation = Quat::from_rotation_z(angle);

    let bullet_bundle = (
        transform,
        LinearVelocity(spawn_info.direction * spawn_info.speed * 0.6),
        RigidBody::Dynamic,
        Collider::circle(BULLET_SIZE),
        Restitution::new(0.8),
        PlayerId(spawn_info.player_id),
        *color,
        BulletMarker { shooter },
        PhysicsProjectile {
            bounce_count: 0,
            max_bounces: 3,
            deceleration: 50.0,
        },
        DespawnAfter(Timer::new(Duration::from_secs(5), TimerMode::Once)),
        Name::new("PhysicsProjectile"),
    );

    if is_predicted {
        commands.spawn((bullet_bundle, PreSpawned::default()));
    } else {
        commands.spawn(bullet_bundle);
    }
}

/// Spawn homing missile as child entity
fn spawn_homing_missile_child(
    commands: &mut Commands,
    spawn_info: &ProjectileSpawn,
    color: &ColorComponent,
    shooter: Entity,
    current_tick: Tick,
    tick_duration: Duration,
    is_predicted: bool,
    is_interpolated: bool,
) {
    let mut position = spawn_info.position;
    let mut transform = Transform::from_translation(position.extend(0.0));

    // For interpolation, adjust spawn position to account for delay
    if is_interpolated {
        let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
        let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
        position += spawn_info.direction * spawn_info.speed * 0.4 * time_elapsed;
        transform.translation = position.extend(0.0);
    }

    let angle = Vec2::new(0.0, 1.0).angle_to(spawn_info.direction);
    transform.rotation = Quat::from_rotation_z(angle);

    let missile_bundle = (
        // TODO: need to add POSITION/ROTATION DIRECTLY!
        transform,
        LinearVelocity(spawn_info.direction * spawn_info.speed * 0.4),
        RigidBody::Kinematic,
        PlayerId(spawn_info.player_id),
        *color,
        BulletMarker { shooter },
        HomingMissile {
            target_entity: None, // TODO: find nearest target
            turn_speed: 2.0,
            acceleration: 100.0,
        },
        DespawnAfter(Timer::new(Duration::from_secs(8), TimerMode::Once)),
        Name::new("HomingMissile"),
    );

    if is_predicted {
        commands.spawn((missile_bundle, PreSpawned::default()));
    } else {
        commands.spawn(missile_bundle);
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
        // TODO: just adding Transform does NOT work, maybe because we disable Transform->Position sync?
        //  or some system ordering issue?
        //  I THINK it's because the SyncSet runs in FixedPostUpdate; so it's possible that we didn't sync Transform to Position
        //  before we replicate Position
        //  for now do NOT spawn Transform, instead directly use Position/Rotation!
        // Transform::from_xyz(0.0, y, 0.0),
        Position::from_xy(0.0, y),
        Rotation::default(),
        ColorComponent(color),
        PlayerMarker,
        Weapon::default(),
        WeaponType::default(),
        Name::new("Player"),
        Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
    )
}
