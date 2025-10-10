use avian2d::prelude::*;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use bevy_enhanced_input::action::Action;
use bevy_enhanced_input::prelude::*;
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client::PeerMetadata;
use lightyear::connection::client_of::ClientOf;
use lightyear::core::tick::TickDuration;
use lightyear::prediction::plugin::PredictionSet;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::LagCompensationSpatialQuery;

use crate::protocol::*;

#[cfg(feature = "server")]
use lightyear::prelude::{Room, RoomEvent};
use lightyear_avian2d::plugin::AvianReplicationMode;

const EPS: f32 = 0.0001;
const BULLET_MOVE_SPEED: f32 = 300.0;
const MAP_LIMIT: f32 = 2000.0;
const HITSCAN_COLLISION_DISTANCE_CHECK: f32 = 2000.0;
const BULLET_COLLISION_DISTANCE_CHECK: f32 = 0.5;

const HITSCAN_LIFETIME: f32 = 0.2;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);

        app.add_observer(rotate_player);
        app.add_observer(move_player);

        // shooting
        app.add_observer(shoot_weapon);
        // - In PreUpdate so that the interpolation tick has been updated (in the previous frame's PostUpdate)
        app.add_systems(PreUpdate, direction_only::handle_projectile_spawn);
        app.add_observer(direction_only::despawn_projectile_spawn);

        // hit detection
        app.add_observer(hit_detection::hitscan_hit_detection);
        app.add_systems(FixedUpdate, hit_detection::bullet_hit_detection);

        app.add_systems(PreUpdate, despawn_after);

        // debug systems
        app.add_systems(FixedLast, fixed_update_log);
        app.add_systems(Last, last_log);

        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            (
                // update_weapon_ring_buffer,
                update_hitscan_visuals,
                update_physics_projectiles,
                update_homing_missiles,
            ),
        );

        // both client and server need physics
        // (the client also needs the physics plugin to be able to compute predicted bullet hits)
        app.add_plugins(lightyear::avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable the position<>transform sync plugins as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));
    }
}

pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

pub(crate) fn rotate_player(
    trigger: On<Fire<MoveCursor>>,
    mut player: Query<(&mut Rotation, &Position), (Without<Bot>, Without<Interpolated>)>,
) {
    if let Ok((mut rotation, position)) = player.get_mut(trigger.context) {
        let angle = Vec2::new(0.0, 1.0).angle_to(trigger.value - position.0);
        // careful to only activate change detection if there was an actual change
        if (angle - rotation.as_radians()).abs() > EPS {
            *rotation = Rotation::from(angle);
        }
    }
}

pub(crate) fn move_player(
    trigger: On<Fire<MovePlayer>>,
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    // Confirmed inputs don't get applied on the client! (for the AllInterpolated case)
    mut player: Query<&mut Position, Without<Interpolated>>,
    is_bot: Query<(), With<Bot>>,
) {
    let (timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
    const PLAYER_MOVE_SPEED: f32 = 1.5;
    if let Ok(mut position) = player.get_mut(trigger.context) {
        if is_bot.get(trigger.context).is_ok() {
            debug!(
                ?tick, ?is_rollback, ?position,
                "Moving player {:?} by {:?}", trigger.context, trigger.value
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
    player: Query<(Entity, &Position), (With<PlayerMarker>, With<PlayerId>)>,
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
    timeline: Single<(&LocalTimeline, Option<&InterpolationTimeline>, Has<Rollback>), Without<ClientOf>>,
    player: Query<
        (Entity, Option<&Position>, &Confirmed<Position>, Option<&Rotation>, Option<&Transform>),
        (With<PlayerMarker>, With<PlayerId>, With<Bot>),
    >,
    interpolated_bullet: Query<
        (Entity, Option<&Position>, &Confirmed<Position>, &ConfirmedHistory<Position>, Option<&Transform>),
        (With<BulletMarker>, With<Interpolated>),
    >,
) {
    let (timeline, interpolation_timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
    let interpolate_tick = interpolation_timeline.map(|t| t.tick());
    for (entity, pos, confirmed, rotation, transform) in player.iter() {
        info!(
            ?tick,
            ?entity,
            ?pos,
            ?confirmed,
            ?rotation,
            transform = ?transform.map(|t| t.translation.truncate()),
            "Player after last"
        );
    }
    for (entity, position, confirmed, history, transform) in interpolated_bullet.iter() {
        debug!(
            ?tick,
            ?interpolate_tick,
            ?entity,
            ?position,
            ?confirmed,
            ?history,
            transform = ?transform.map(|t| t.translation.truncate()),
            "Bullet after fixed update"
        );
    }
}

/// Main weapon shooting system that handles all weapon types
pub(crate) fn shoot_weapon(
    trigger: On<Complete<Shoot>>,
    mut commands: Commands,
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    time: Res<Time>,
    tick_duration: Res<TickDuration>,
    query: SpatialQuery,
    mut player_query: Query<
        (
            &PlayerId,
            // have to use Position/Rotation, as we don't do PositionToTransform until PostUpdate so the Transform is not accurate
            &Position,
            &Rotation,
            &ColorComponent,
            &mut Weapon,
            Option<&ControlledBy>,
        ),
        With<PlayerMarker>,
    >,
    global: Single<
        (
            &ProjectileReplicationMode,
            &GameReplicationMode,
            &WeaponType,
        ),
        With<ClientContext>,
    >,
) {
    let tick = timeline.tick();
    let tick_duration = tick_duration.0;
    let shooter = trigger.context;
    let (projectile_mode, replication_mode, weapon_type) = global.into_inner();

    if let Ok((id, position, rotation, color, mut weapon, controlled_by)) = player_query.get_mut(shooter) {
        let is_server = controlled_by.is_some();
        // Check fire rate
        if let Some(last_fire) = weapon.last_fire_tick {
            let ticks_since_last_fire = tick.0.saturating_sub(last_fire.0);
            let time_since_last_fire = Duration::from_secs_f64(ticks_since_last_fire as f64 / 64.0);
            let min_fire_interval = Duration::from_secs_f32(1.0 / weapon_type.fire_rate());

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
                full_entity::shoot_with_full_entity_replication(
                    &mut commands,
                    &timeline,
                    position,
                    rotation,
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
                direction_only::shoot_with_direction_only_replication(
                    &mut commands,
                    &timeline,
                    position,
                    rotation,
                    id,
                    shooter,
                    color,
                    controlled_by,
                    is_server,
                    replication_mode,
                    weapon_type,
                );
            }
            // (_, ProjectileReplicationMode::RingBuffer) => {
            //     shoot_with_ring_buffer_replication(
            //         &mut weapon,
            //         &timeline,
            //         transform,
            //         id,
            //         shooter,
            //         weapon_type,
            //     );
            // }
        }
    }
}

/// Ring buffer replication - store projectiles in weapon component
fn shoot_with_ring_buffer_replication(
    weapon: &mut Weapon,
    timeline: &LocalTimeline,
    position: &Position,
    rotation: &Rotation,
    id: &PlayerId,
    shooter: Entity,
    weapon_type: &WeaponType,
) {
    let projectile_info = ProjectileSpawnInfo {
        spawn_tick: timeline.tick(),
        position: *position,
        rotation: *rotation,
        weapon_type: *weapon_type,
    };

    // Add to ring buffer
    weapon.projectile_buffer.push(projectile_info);
    if weapon.projectile_buffer.len() > weapon.buffer_capacity {
        weapon.projectile_buffer.remove(0); // Remove oldest
    }
}

mod hit_detection {
    use super::*;
    pub(crate) fn hitscan_hit_detection(
        trigger: On<Add, HitscanVisual>,
        commands: Commands,
        server: Query<Entity, With<Server>>,
        timeline: Query<&LocalTimeline, Without<ClientOf>>,
        mode: Query<&GameReplicationMode, With<ClientContext>>,
        mut spatial_set: ParamSet<(LagCompensationSpatialQuery, SpatialQuery)>,
        bullet: Query<(&HitscanVisual, &BulletMarker, &PlayerId)>,
        target_query: Query<&GameReplicationMode, With<PlayerMarker>>,
        // the InterpolationDelay component is stored directly on the client entity
        // (the server creates one entity for each client to store client-specific
        // metadata)
        client_query: Query<&InterpolationDelay, With<ClientOf>>,
        mut hit_sender: Query<(&LocalId, &mut EventSender<HitDetected>), With<Client>>,
        mut player_query: Query<AnyOf<(&mut Score, &ControlledBy, &Predicted)>, With<PlayerMarker>>,
    ) {
        let Ok(timeline) = timeline.single() else {
            info!("no unique timeline");
            return;
        };
        let Ok(mode) = mode.single() else {
            info!("no unique mode");
            return;
        };
        let Ok((hitscan, bullet_marker, id)) = bullet.get(trigger.entity) else {
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
            let (local_id, _) = hit_sender.single_mut().unwrap();
            if mode == &GameReplicationMode::ClientSideHitDetection && id.0 != local_id.0 {
                // for client-side hit detection, we only tell the server about hits from our own bullets
                return;
            }
            // TODO: ignore bullets that were fired by other clients
        }
        info!(?hitscan, "Hit detection for hitscan");

        match mode {
            GameReplicationMode::ClientPredictedLagComp => {
                let Ok(Some(controlled_by)) = player_query
                    .get(shooter)
                    .map(|(_, controlled_by, _)| controlled_by)
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
                    true,
                    // we stop on the first time the predicate is true, i.e. check if we hit a player that is in
                    // the same room (GameReplicationMode) and has a PlayerMarker
                    // this is important to not hit the lag compensation colliders
                    &|entity| target_query.get(entity).is_ok_and(|m| m == mode),
                    // avoid hitting ourselves
                    &mut SpatialQueryFilter::from_excluded_entities([shooter]),
                ) {
                    let target = hit_data.entity;
                    info!(?tick, ?hit_data, ?shooter, ?target, "Hitscan hit detected");
                    // if there is a hit, increment the score
                    if let Ok((Some(mut score), _, _)) = player_query.get_mut(shooter) {
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
                    true,
                    &mut SpatialQueryFilter::from_excluded_entities([shooter]),
                    // we stop on the first time the predicate is true, i.e. check if we hit a player that is in
                    // the same room (GameReplicationMode) and has a PlayerMarker
                    // this is important to not hit the lag compensation colliders
                    &|entity| target_query.get(entity).is_ok_and(|m| m == mode),
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
                    if let Ok((Some(mut score), _, _)) = player_query.get_mut(shooter) {
                        info!("Increment score");
                        score.0 += 1;
                    }
                    // client-side hit detection: the client needs to notify the server about the hit
                    if !is_server
                        && mode == &GameReplicationMode::ClientSideHitDetection
                        && let Ok((_, mut sender)) = hit_sender.single_mut()
                    {
                        info!("Client detected hit! Sending hit detection trigger to server");
                        sender.trigger::<HitChannel>(HitDetected { shooter, target });
                    }
                }
            }
        }
    }

    /// Hit detection for full-entity bullets
    pub(crate) fn bullet_hit_detection(
        mut commands: Commands,
        server: Query<Entity, With<Server>>,
        timeline: Single<&LocalTimeline, Without<ClientOf>>,
        mode: Single<&GameReplicationMode, With<ClientContext>>,
        mut spatial_set: ParamSet<(LagCompensationSpatialQuery, SpatialQuery)>,
        bullet: Query<(Entity, &Position, &LinearVelocity, &BulletMarker, &PlayerId)>,
        target_query: Query<&GameReplicationMode, With<PlayerMarker>>,
        // the InterpolationDelay component is stored directly on the client entity
        // (the server creates one entity for each client to store client-specific
        // metadata)
        client_query: Query<&InterpolationDelay, With<ClientOf>>,
        mut hit_sender: Query<(&LocalId, &mut EventSender<HitDetected>), With<Client>>,
        mut player_query: Query<AnyOf<(&mut Score, &ControlledBy, &Predicted)>, With<PlayerMarker>>,
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
        }
        bullet
            .iter()
            .for_each(|(entity, position, velocity, bullet_marker, id)| {
                if mode == &GameReplicationMode::ClientSideHitDetection {
                    let (local_id, _) = hit_sender.single_mut().unwrap();
                    if id.0 != local_id.0 {
                        // for client-side hit detection, we only tell the server about hits from our own bullets
                        return;
                    }
                }
                let shooter = bullet_marker.shooter;
                let Some(direction) = velocity.0.try_normalize() else {
                    info!(
                        ?is_server,
                        "Despawning bullet {entity:?} with invalid velocity {velocity:?}"
                    );
                    commands.entity(entity).try_despawn();
                    return;
                };
                let start = position.0;
                let max_distance = BULLET_COLLISION_DISTANCE_CHECK;

                match mode {
                    GameReplicationMode::ClientPredictedLagComp => {
                        let Ok(Some(controlled)) = player_query
                            .get(shooter)
                            .map(|(_, controlled_by, _)| controlled_by)
                        else {
                            error!("Could not retrieve controlled_by for client {id:?}");
                            return;
                        };
                        let Ok(delay) = client_query.get(controlled.owner) else {
                            error!("Could not retrieve InterpolationDelay for client {id:?}");
                            return;
                        };
                        let query = spatial_set.p0();
                        if let Some(hit_data) = query.cast_ray_predicate(
                            // the delay is sent in every input message; the latest InterpolationDelay received
                            // is stored on the client entity
                            *delay,
                            start,
                            Dir2::new_unchecked(direction),
                            max_distance,
                            true,
                            // we stop on the first time the predicate is true, i.e. check if we hit a player that is in
                            // the same room (GameReplicationMode) and has a PlayerMarker
                            // this is important to not hit the lag compensation colliders
                            &|entity| target_query.get(entity).is_ok_and(|m| m == mode),
                            &mut SpatialQueryFilter::from_excluded_entities([shooter]),
                        ) {
                            // TODO: the client should also predict the hit so that it can show some cosmetics and despawn the bullet!
                            let target = hit_data.entity;
                            info!(?tick, ?hit_data, ?shooter, ?target, "Bullet hit detected");
                            commands.entity(entity).try_despawn();
                            // if there is a hit, increment the score
                            if let Ok((Some(mut score), _, _)) = player_query.get_mut(shooter) {
                                info!("Increment score");
                                score.0 += 1;
                            }
                        }
                    }
                    _ => {
                        let query = spatial_set.p1();
                        if let Some(hit_data) = query.cast_ray_predicate(
                            start,
                            Dir2::new_unchecked(direction),
                            max_distance,
                            true,
                            &mut SpatialQueryFilter::from_excluded_entities([shooter]),
                            // we stop on the first time the predicate is true, i.e. check if we hit a player that is in
                            // the same room (GameReplicationMode) and has a PlayerMarker
                            // this is important to not hit the lag compensation colliders
                            &|entity| target_query.get(entity).is_ok_and(|m| m == mode),
                        ) {
                            let target = hit_data.entity;
                            info!(
                                ?mode,
                                ?tick,
                                ?hit_data,
                                ?shooter,
                                ?target,
                                "Bullet hit detected"
                            );
                            // TODO: this might not be enough because in ClientSideHitDetection the server might re-replicate the bullet?
                            commands.entity(entity).try_despawn();
                            // if there is a hit, increment the score
                            if let Ok((Some(mut score), _, _)) = player_query.get_mut(shooter) {
                                info!("Increment score");
                                score.0 += 1;
                            }
                            // client-side hit detection: the client needs to notify the server about the hit
                            if !is_server
                                && mode == &GameReplicationMode::ClientSideHitDetection
                                && let Ok((_, mut sender)) = hit_sender.single_mut()
                            {
                                info!(
                                    "Client detected hit! Sending hit detection trigger to server"
                                );
                                sender.trigger::<HitChannel>(HitDetected { shooter, target });
                            }
                        }
                    }
                }
            });
    }
}

mod full_entity {
    use core::f32::consts::PI;
    use super::*;

    /// Full entity replication: spawn a replicated entity for the projectile
    /// The entity keeps getting replicated from server to clients
    pub(super) fn shoot_with_full_entity_replication(
        commands: &mut Commands,
        timeline: &LocalTimeline,
        position: &Position,
        rotation: &Rotation,
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
                    position,
                    rotation,
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
                    position,
                    rotation,
                    id,
                    shooter,
                    color,
                    controlled_by,
                    replication_mode,
                    is_server,
                );
            }
            // WeaponType::Shotgun => {
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
            // }
            // WeaponType::PhysicsProjectile => {
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
            // }
            // WeaponType::HomingMissile => {
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
            // }
        }
    }
    fn shoot_hitscan(
        commands: &mut Commands,
        timeline: &LocalTimeline,
        position: &Position,
        rotation: &Rotation,
        id: &PlayerId,
        shooter: Entity,
        color: &ColorComponent,
        controlled_by: Option<&ControlledBy>,
        replication_mode: &GameReplicationMode,
    ) {
        let tick = timeline.tick();
        let is_server = controlled_by.is_some();
        let up = Vec2::new(0.0, 1.0);
        let start = position.0;
        let end = start + rotation * up * 1000.0; // Long hitscan range

        // For Hitscan, we directly spawn an entity that represents the 'bullet'
        let spawn_bundle = (
            HitscanVisual {
                start,
                end,
                lifetime: 0.0,
                max_lifetime: HITSCAN_LIFETIME,
            },
            // we add BulletMarker to idenfity who the shooter is
            BulletMarker { shooter },
            *color,
            *id,
            Name::new("HitscanProjectileSpawn"),
        );
        info!(?is_server, ?tick, ?shooter, "FullEntity Hitscan shoot");
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
                    ));
                }
                GameReplicationMode::AllInterpolated => {
                    commands.spawn((
                        spawn_bundle,
                        Replicate::to_clients(NetworkTarget::All),
                        InterpolationTarget::to_clients(NetworkTarget::All),
                        controlled_by.unwrap().clone(),
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
        position: &Position,
        rotation: &Rotation,
        id: &PlayerId,
        shooter: Entity,
        color: &ColorComponent,
        controlled_by: Option<&ControlledBy>,
        replication_mode: &GameReplicationMode,
        is_server: bool,
    ) {
        let velocity = LinearVelocity(*rotation * Vec2::new(0.0, 1.0) * BULLET_MOVE_SPEED);
        let bullet_bundle = (
            *position,
            *rotation,
            velocity,
            DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
            RigidBody::Kinematic,
            *id,
            *color,
            BulletMarker { shooter },
            Name::new("LinearProjectile"),
        );
        info!(?bullet_bundle, "Shooting FullEntity LinearProjectile");
        if is_server {
            #[cfg(feature = "server")]
            match replication_mode {
                GameReplicationMode::AllPredicted => {
                    // We do not predict other players shooting? or should we do it if we received the input in time?
                    commands.spawn((
                        bullet_bundle,
                        // we predict-spawn the bullet on the client, so we need to also add PreSpawned on the server
                        PreSpawned::default(),
                        Replicate::to_clients(NetworkTarget::All),
                        PredictionTarget::to_clients(NetworkTarget::All),
                        controlled_by.unwrap().clone(),
                    ));
                }
                GameReplicationMode::ClientPredictedNoComp
                | GameReplicationMode::ClientPredictedLagComp
                | GameReplicationMode::ClientSideHitDetection => {
                    commands.spawn((
                        bullet_bundle,
                        // we predict-spawn the bullet on the client, so we need to also add PreSpawned on the server
                        PreSpawned::default(),
                        Replicate::to_clients(NetworkTarget::All),
                        PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                        // only replicate RigidBody to the shooter. We don't want interpolated bullets to have RigidBody
                        ComponentReplicationOverrides::<RigidBody>::default().disable_all().enable_for(shooter),
                        controlled_by.unwrap().clone(),
                    ));
                }
                GameReplicationMode::AllInterpolated => {
                    commands.spawn((
                        bullet_bundle,
                        Replicate::to_clients(NetworkTarget::All),
                        InterpolationTarget::to_clients(NetworkTarget::All),
                        // We don't want interpolated bullets to have RigidBody
                        ComponentReplicationOverrides::<RigidBody>::default().disable_all(),
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
                | GameReplicationMode::ClientPredictedLagComp
                | GameReplicationMode::ClientSideHitDetection => {
                    commands.spawn((bullet_bundle, PreSpawned::default()));
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
        position: &Position,
        rotation: &Rotation,
        id: &PlayerId,
        shooter: Entity,
        color: &ColorComponent,
        controlled_by: Option<&ControlledBy>,
        replication_mode: &GameReplicationMode,
        is_server: bool,
    ) {
        let pellet_count = 8;
        let spread_angle = PI / 6.0; // 30 degrees spread

        for i in 0..pellet_count {
            let angle_offset = (i as f32 - (pellet_count - 1) as f32 / 2.0) * spread_angle
                / (pellet_count - 1) as f32;

            let mut new_rotation = rotation.add_angle_fast(angle_offset);
            let velocity = LinearVelocity(new_rotation * Vec2::new(0.0, BULLET_MOVE_SPEED * 0.8));

            let pellet_bundle = (
                *position,
                new_rotation,
                velocity,
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
        position: &Position,
        rotation: &Rotation,
        id: &PlayerId,
        shooter: Entity,
        color: &ColorComponent,
        controlled_by: Option<&ControlledBy>,
        replication_mode: &GameReplicationMode,
        is_server: bool,
    ) {
        let bullet_bundle = (
            *position,
            *rotation,
            LinearVelocity(*rotation * Vec2::new(0.0, BULLET_MOVE_SPEED * 0.6)),
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
        position: &Position,
        rotation: &Rotation,
        id: &PlayerId,
        shooter: Entity,
        color: &ColorComponent,
        controlled_by: Option<&ControlledBy>,
        is_server: bool,
        target: Option<Entity>,
    ) {
        let missile_bundle = (
            *position,
            *rotation,
            LinearVelocity(*rotation * Vec2::new(0.0, BULLET_MOVE_SPEED * 0.4)),
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
}

/// To save bandwidth, the server only sends a message 'Client Fired Bullet'
pub(crate) mod direction_only {
    use core::f32::consts::PI;
    use super::*;

    /// Identifies the ProjectileSpawn that spawned the bullet
    #[derive(Component, Debug)]
    #[relationship(relationship_target = Bullets)]
    pub struct BulletOf(Entity);

    /// Bullets spawned by a ProjectileSpawn
    #[derive(Component, Debug)]
    #[relationship_target(relationship = BulletOf, linked_spawn)]
    pub struct Bullets(Vec<Entity>);

    /// Direction-only replication - only replicate spawn parameters
    pub(crate) fn shoot_with_direction_only_replication(
        commands: &mut Commands,
        timeline: &LocalTimeline,
        position: &Position,
        rotation: &Rotation,
        id: &PlayerId,
        shooter: Entity,
        color: &ColorComponent,
        controlled_by: Option<&ControlledBy>,
        is_server: bool,
        replication_mode: &GameReplicationMode,
        weapon_type: &WeaponType,
    ) {
        let speed = match weapon_type {
            WeaponType::Hitscan => 1000.0, // Instant
            WeaponType::LinearProjectile => BULLET_MOVE_SPEED,
            // WeaponType::Shotgun => BULLET_MOVE_SPEED * 0.8,
            // WeaponType::PhysicsProjectile => BULLET_MOVE_SPEED * 0.6,
            // WeaponType::HomingMissile => BULLET_MOVE_SPEED * 0.4,
        };

        // TODO: for hitscan, maybe we can just do similar to FullEntity replication? We add HitscanVisual
        let spawn_info = ProjectileSpawn {
            spawn_tick: timeline.tick(),
            position: *position,
            rotation: *rotation,
            speed,
            color: *color,
            weapon_type: *weapon_type,
            shooter,
            player_id: id.0,
        };
        info!(?spawn_info, "Shooting ProjectileSpawn");
        let spawn_bundle = (spawn_info, Name::new("ProjectileSpawn"));

        // TODO: instead of replicating an entity; we can just send a one-off message?
        //  but how to do prediction?
        // TODO: with entity:
        //  - prediction: client predicted the SpawnInfo entity and spawned children projectile entities
        //     if we were mispredicting and the client didn't shoot, then we have to despawn all the predicted projectiles
        //  - interpolation: ideally the client interpolates by adjusting the position to account for the exact fire tick

        if is_server {
            #[cfg(feature = "server")]
            match replication_mode {
                // - Server spawns ProjectileSpawn
                //   - Spawns a child entity with the bullet that is not replicated
                //   - when the child dies we need to despawn the parent
                // - Client spawns a PreSpawned ProjectileSpawn
                //   - Spawns a child entity with the bullet
                // -> On PreSpawned mismatch, the ProjectileSpawn entity will be despawned, so the child bullets too
                // -> On PreSpawned match, we get a Confirmed ProjectileSpawn and a Predicted ProjectileSpawn that has a child.
                // -> InterestManagement can be applied since it's an entity
                GameReplicationMode::AllPredicted => {
                    commands.spawn((
                        spawn_bundle,
                        Replicate::to_clients(NetworkTarget::All),
                        PredictionTarget::to_clients(NetworkTarget::All),
                        controlled_by.unwrap().clone(),
                        PreSpawned::default(),
                    ));
                }
                GameReplicationMode::ClientPredictedNoComp
                | GameReplicationMode::ClientPredictedLagComp
                | GameReplicationMode::ClientSideHitDetection => {
                    commands.spawn((
                        spawn_bundle,
                        Replicate::to_clients(NetworkTarget::All),
                        PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                        PreSpawned::default(),
                        controlled_by.unwrap().clone(),
                    ));
                }
                GameReplicationMode::AllInterpolated => {
                    commands.spawn((
                        spawn_bundle,
                        Replicate::to_clients(NetworkTarget::All),
                        InterpolationTarget::to_clients(NetworkTarget::All),
                        PreSpawned::default(),
                        controlled_by.unwrap().clone(),
                    ));
                }
                GameReplicationMode::OnlyInputsReplicated => {}
            }
        } else {
            match replication_mode {
                GameReplicationMode::AllPredicted => {
                    // TODO: if there is a mismatch, how do we make sure that the bullet gets corrected?
                    //  e.g. the server has a ProjectileSpawn data, the client used a different ProjectileSpawn data which doesn't match (because for example its initial position incorrect)
                    //  - we could set PredictionMode::Full for ProjectileSpawn, but if there is a rollback we would simply update the value of ProjectileSpawn, without re-triggering an OnAdd observer
                    //  - we want the previous Bullet(s) to be despawned, so that on rollback we re-shoot a bullet on the client from the new position.
                    //    Maybe we can let components register a custom `prepare_rollback` fn? Or emit a custom Rollback event for each entity/component that was rolled back? That way the client could use that
                    //    to despawn the Bullets.
                    // should we predict other clients shooting?
                    commands.spawn((spawn_bundle, PreSpawned::default()));
                }
                GameReplicationMode::ClientPredictedNoComp
                | GameReplicationMode::ClientPredictedLagComp
                | GameReplicationMode::ClientSideHitDetection => {
                    commands.spawn((spawn_bundle, PreSpawned::default()));
                }
                GameReplicationMode::AllInterpolated => {
                    // TODO: we need to spawn the projectile at the correct interpolation delay on the client
                    //  i.e. if the shot was at tick T, then we should only spawn once the interpolation_timeline
                    //  reaches tick t. -> store a buffer of ProjectileSpawn to spawn.

                    // OR: the client receives the ProjectileSpawn. Since there is no PreSpawned it knows it's interpolated
                    //  and it spawns the bullet at the correct offset from the player ?
                }
                GameReplicationMode::OnlyInputsReplicated => {
                    commands.spawn((spawn_bundle, DeterministicPredicted));
                }
            }
        }
    }

    /// Handle ProjectileSpawn by spawning child entities for projectiles
    ///
    /// - Prediction: the client will PreSpawn the ProjectileSpawn entity, at which point they will locally spawn
    ///    a bullet. Then they will receive the Replicated (ProjectileSpawn, Prespawned) from the server which will match
    ///    with their prespawned ProjectileSpawn.
    ///
    /// - Interpolation: the client receives a Replicated (Confirmed) and Interpolated entities with ProjectileSpawn.
    ///     The Replicated entity doesn't spawn any bullet.
    ///     The Interpolated entity spawns a bullet only when the interpolation tick reaches the shooter tick.
    ///
    ///   NOTE: this could have been an observer if we were only handling predicted spawns. For interpolated spawns
    ///     we need to make it a system because we need to check the interpolation_tick every tick
    pub(crate) fn handle_projectile_spawn(
        mut commands: Commands,
        timeline: Single<(&LocalTimeline, Option<&InterpolationTimeline>), Without<ClientOf>>,
        tick_duration: Res<TickDuration>,
        spawn_query: Query<
            (Entity, &ProjectileSpawn, Has<Interpolated>),
            // avoid spawning bullets multiple times for one ProjectileSpawn
            Without<Bullets>,
        >,
    ) {
        let (local_timeline, interpolated_timeline) = timeline.into_inner();
        let current_tick = local_timeline.tick();
        spawn_query
            .iter()
            .for_each(|(entity, spawn_info, interpolated)| {
                // TODO: account for interpolation overstep?
                // in the interpolated case, we wait until the interpolation tick has been reached to spawn the bullet
                if interpolated
                    && let Some(interpolation_tick) = interpolated_timeline.map(|t| t.tick())
                    && interpolation_tick < spawn_info.spawn_tick
                {
                    info!(?interpolation_tick, "Waiting for interpolation_tick to spawn ProjectileSpawn with spawn tick {:?}", spawn_info.spawn_tick);
                    return;
                }
                match spawn_info.weapon_type {
                    WeaponType::Hitscan => {
                        // TODO: spawn the hitscan at the right time on the Interpolation Timeline!
                        // Create hitscan visual child entity
                        spawn_hitscan_visual(&mut commands, spawn_info, entity, current_tick);
                    }
                    WeaponType::LinearProjectile => {
                        // Create linear projectile child entity
                        spawn_linear_projectile_child(
                            &mut commands,
                            spawn_info,
                            entity,
                            current_tick,
                            tick_duration.0,
                        );
                    }
                    // WeaponType::Shotgun => {
                        // // Create shotgun pellets
                        // spawn_shotgun_pellets(
                        //     &mut commands,
                        //     spawn_info,
                        //     color,
                        //     shooter,
                        //     current_tick,
                        //     tick_duration.0,
                        //     is_predicted,
                        //     is_interpolated,
                        // );
                    // }
                    // WeaponType::PhysicsProjectile => {
                        // // Create physics projectile child entity
                        // spawn_physics_projectile_child(
                        //     &mut commands,
                        //     spawn_info,
                        //     color,
                        //     shooter,
                        //     current_tick,
                        //     tick_duration.0,
                        //     is_predicted,
                        //     is_interpolated,
                        // );
                    // }
                    // WeaponType::HomingMissile => {
                        // // Create homing missile child entity
                        // spawn_homing_missile_child(
                        //     &mut commands,
                        //     spawn_info,
                        //     color,
                        //     shooter,
                        //     current_tick,
                        //     tick_duration.0,
                        //     is_predicted,
                        //     is_interpolated,
                        // );
                    // }
                }
            });
    }

    /// When the child bullet gets despawned, despawn the parent ProjectileSpawn entity
    pub(crate) fn despawn_projectile_spawn(
        trigger: On<Remove, BulletOf>,
        bullet: Query<&BulletOf, With<BulletMarker>>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = bullet.get(trigger.entity)
            && let Ok(mut c) = commands.get_entity(child_of.0)
        {
            c.try_despawn();
        }
    }

    /// Spawn hitscan visual as child entity of the ProjectileSpawn entity
    fn spawn_hitscan_visual(
        commands: &mut Commands,
        spawn_info: &ProjectileSpawn,
        spawn_entity: Entity,
        current_tick: Tick,
    ) {
        let start = spawn_info.position.0;
        let end = spawn_info.rotation * Vec2::new(0.0, 1000.0);

        let visual_bundle = (
            HitscanVisual {
                start,
                end,
                lifetime: 0.0,
                max_lifetime: HITSCAN_LIFETIME,
            },
            BulletMarker {
                shooter: spawn_info.shooter,
            },
            spawn_info.color,
            PlayerId(spawn_info.player_id),
            BulletOf(spawn_entity),
            DisableRollback,
            Name::new("HitscanVisual"),
        );
        info!("Spawning hitscan visual: {:?}", visual_bundle);
        commands.spawn(visual_bundle);
    }

    /// Spawn linear projectile as child entity
    fn spawn_linear_projectile_child(
        commands: &mut Commands,
        spawn_info: &ProjectileSpawn,
        spawn_entity: Entity,
        current_tick: Tick,
        tick_duration: Duration,
    ) {
        let position = spawn_info.position;
        let rotation = spawn_info.rotation;

        // // For interpolation, adjust spawn position to account for delay
        // if is_interpolated {
        //     let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
        //     let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
        //     position += spawn_info.direction * spawn_info.speed * time_elapsed;
        //     transform.translation = position.extend(0.0);
        // }

        // transform.rotation = Quat::from_rotation_z(angle);
        info!(?current_tick, ?position, "Spawning DirectionOnly LinearProjectile");

        let bullet_bundle = (
            position,
            rotation,
            LinearVelocity(rotation * Vec2::new(0.0, spawn_info.speed)),
            RigidBody::Kinematic,
            PlayerId(spawn_info.player_id),
            spawn_info.color,
            BulletMarker {
                shooter: spawn_info.shooter,
            },
            DespawnAfter(Timer::new(Duration::from_secs(3), TimerMode::Once)),
            // The entity is not predicted, so we want to disable it during rollbacks, otherwise it will start
            // jumping forward on rollbacks.
            BulletOf(spawn_entity),
            DisableRollback,
            Name::new("LinearProjectile"),
        );
        // the bullet itself is not PreSpawned, its parent entity is
        commands.spawn(bullet_bundle);
    }

    /// Spawn shotgun pellets as child entities
    fn spawn_shotgun_pellets(
        commands: &mut Commands,
        spawn_info: &ProjectileSpawn,
        spawn_entity: Entity,
        current_tick: Tick,
        tick_duration: Duration,
    ) {
        let pellet_count = 8;
        let spread_angle = PI / 6.0; // 30 degrees spread

        let position = spawn_info.position;
        let rotation = spawn_info.rotation;

        for i in 0..pellet_count {
            let angle_offset = (i as f32 - (pellet_count - 1) as f32 / 2.0) * spread_angle
                / (pellet_count - 1) as f32;

            let new_rotation = rotation.add_angle_fast(angle_offset);

            // // For interpolation, adjust spawn position to account for delay
            // if is_interpolated {
            //     let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
            //     let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
            //     position += pellet_direction * spawn_info.speed * 0.8 * time_elapsed;
            //     transform.translation = position.extend(0.0);
            // }

            let pellet_bundle = (
                position,
                new_rotation,
                LinearVelocity(new_rotation * Vec2::new(0.0, spawn_info.speed * 0.8)),
                RigidBody::Kinematic,
                PlayerId(spawn_info.player_id),
                spawn_info.color,
                BulletMarker {
                    shooter: spawn_info.shooter,
                },
                ShotgunPellet {
                    pellet_index: i,
                    spread_angle: angle_offset,
                },
                DespawnAfter(Timer::new(Duration::from_secs(2), TimerMode::Once)),
                BulletOf(spawn_entity),
                DisableRollback,
                Name::new("ShotgunPellet"),
            );

            commands.spawn(pellet_bundle);
        }
    }

    /// Spawn physics projectile as child entity
    fn spawn_physics_projectile_child(
        commands: &mut Commands,
        spawn_info: &ProjectileSpawn,
        spawn_entity: Entity,
        current_tick: Tick,
        tick_duration: Duration,
    ) {
        // // For interpolation, adjust spawn position to account for delay
        // if is_interpolated {
        //     let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
        //     let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
        //     position += spawn_info.direction * spawn_info.speed * 0.6 * time_elapsed;
        //     transform.translation = position.extend(0.0);
        // }

        let position = spawn_info.position;
        let rotation = spawn_info.rotation;

        let bullet_bundle = (
            position,
            rotation,
            LinearVelocity(rotation * Vec2::new(0.0, spawn_info.speed * 0.6)),
            RigidBody::Dynamic,
            Collider::circle(BULLET_SIZE),
            Restitution::new(0.8),
            PlayerId(spawn_info.player_id),
            spawn_info.color,
            BulletMarker {
                shooter: spawn_info.shooter,
            },
            PhysicsProjectile {
                bounce_count: 0,
                max_bounces: 3,
                deceleration: 50.0,
            },
            DespawnAfter(Timer::new(Duration::from_secs(5), TimerMode::Once)),
            BulletOf(spawn_entity),
            DisableRollback,
            Name::new("PhysicsProjectile"),
        );

        commands.spawn(bullet_bundle);
    }

    /// Spawn homing missile as child entity
    fn spawn_homing_missile_child(
        commands: &mut Commands,
        spawn_info: &ProjectileSpawn,
        spawn_entity: Entity,
        current_tick: Tick,
        tick_duration: Duration,
    ) {
        // For interpolation, adjust spawn position to account for delay
        // if is_interpolated {
        //     let ticks_elapsed = current_tick.0.saturating_sub(spawn_info.spawn_tick.0);
        //     let time_elapsed = ticks_elapsed as f32 * tick_duration.as_secs_f32();
        //     position += spawn_info.direction * spawn_info.speed * 0.4 * time_elapsed;
        //     transform.translation = position.extend(0.0);
        // }

        let position = spawn_info.position;
        let rotation = spawn_info.rotation;
        let missile_bundle = (
            position,
            rotation,
            LinearVelocity(rotation * Vec2::new(0.0, spawn_info.speed * 0.4)),
            RigidBody::Kinematic,
            PlayerId(spawn_info.player_id),
            spawn_info.color,
            BulletMarker {
                shooter: spawn_info.shooter,
            },
            HomingMissile {
                target_entity: None, // TODO: find nearest target
                turn_speed: 2.0,
                acceleration: 100.0,
            },
            DespawnAfter(Timer::new(Duration::from_secs(8), TimerMode::Once)),
            BulletOf(spawn_entity),
            DisableRollback,
            Name::new("HomingMissile"),
        );
        commands.spawn(missile_bundle);
    }
}

/// The direction-only method still involves replicating a new entity for each bullet.
/// That can be expensive because of:
/// - entity mapping has to be done
/// - new entities are replicated via a reliable channel
/// - a temporary 'fake' entity has to be spawned
///
/// Instead we can use a ring buffer Component on the shooter entity that contains the list of projectiles to shoot
/// The client gets replicated this ring buffer unreliably and maintains an index of the projectiles it already shot.
///
/// We still get the benefits of world replication: interest management is enabled.
mod ring_buffer {
    use super::*;

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
        let speed = match projectile_info.weapon_type {
            WeaponType::LinearProjectile => BULLET_MOVE_SPEED,
            // WeaponType::Shotgun => BULLET_MOVE_SPEED * 0.8,
            // WeaponType::PhysicsProjectile => BULLET_MOVE_SPEED * 0.6,
            // WeaponType::HomingMissile => BULLET_MOVE_SPEED * 0.4,
            _ => BULLET_MOVE_SPEED, // Default for hitscan weapons (though they shouldn't use ring buffer)
        };
        let position = projectile_info.position;
        let rotation = projectile_info.rotation;
        let bullet_bundle = (
            position,
            rotation,
            LinearVelocity(rotation * Vec2::new(0.0, speed)),
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
            info!(?entity, "despawn hitscan");
            commands.entity(entity).try_despawn();
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

#[derive(Component, Clone, PartialEq, Debug)]
pub struct DespawnAfter(pub Timer);

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
        if despawn_after.0.is_finished() {
            commands.entity(entity).try_despawn();
        }
    }
}

pub fn player_bundle(client_id: PeerId, mode: GameReplicationMode) -> impl Bundle {
    let y = (client_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    let color = color_from_id(client_id);
    (
        // the context needs to be inserted on the server, and will be replicated to the client
        PlayerContext,
        Score(0),
        PlayerId(client_id),
        RigidBody::Kinematic,
        Position::from_xy(0.0, y),
        Rotation::default(),
        ColorComponent(color),
        PlayerMarker,
        Weapon::default(),
        Name::new("Player"),
        Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
        // Track the replication mode of the player
        mode,
    )
}
