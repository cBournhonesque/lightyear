//! Rewound server hit detection.
//!
//! The server remains authoritative but evaluates target colliders at the
//! historical interpolation delay reported with the shooter's inputs.
//!
//! # Advantages
//!
//! - Makes the authoritative query agree more closely with what an
//!   interpolating shooter saw.
//! - The server still owns damage, cadence checks, and final hit selection.
//! - Provides a direct comparison with current-state server queries.
//!
//! # Trade-offs
//!
//! - Requires retained collider history, careful schedule ordering, and
//!   explicit handling when client timing metadata is unavailable.
//! - Gives the shooter an intentional latency-related advantage over the
//!   target and therefore needs a bounded maximum rewind.
//! - The current Lightyear API uses the latest input interpolation delay. A
//!   future refinement should attach an exact view timestamp to each shot.

use avian2d::prelude::*;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use lightyear::connection::client_of::ClientOf;
use lightyear::connection::host::HostClient;
use lightyear::interpolation::plugin::InterpolationDelay;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::{LagCompensationRayHit, LagCompensationSpatialQuery};

use super::{AuthoritativeProjectile, HitPolicy, remember_impact};
use crate::protocol::{BulletMarker, ClientContext, PlayerId, PlayerMarker, Score};
use crate::representation::shot_buffer::{
    BufferedProjectileOf, BufferedSequence, ShotBuffer, finish_linear_projectile,
};
use crate::shared::DespawnAfter;
use crate::trajectory::{hitscan, linear};

/// A short-lived, server-only record of a historical collider pose tested by
/// a lag-compensated query, whether that query hits or misses.
///
/// The renderer draws this as a yellow outline. Keeping it as ordinary ECS
/// data (instead of drawing directly in the fixed-tick hit system) makes the
/// sample remain visible for long enough to inspect and keeps hit detection
/// independent from rendering.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct LagCompensatedSilhouette {
    pub(crate) shooter: Entity,
    pub(crate) target: Entity,
    pub(crate) position: Vec2,
    pub(crate) rotation: f32,
    pub(crate) sample_tick: Tick,
    pub(crate) sample_overstep: f32,
}

const SILHOUETTE_LIFETIME: f32 = 0.65;

fn remember_hit(
    commands: &mut Commands,
    origin: Vec2,
    direction: Dir2,
    result: LagCompensationRayHit,
) {
    remember_impact(commands, origin, direction, result.hit);

    debug!(
        target = ?result.hit.entity,
        sample_tick = ?result.interpolation_tick,
        sample_overstep = result.interpolation_overstep,
        position = ?result.position,
        rotation = ?result.rotation,
        "Lag-compensated hit used this historical target pose"
    );
}

/// Show every historical target pose tested for one shooter's query, including
/// misses. Reusing one debug entity per shooter/target pair avoids leaving a
/// trail of rectangles while a linear projectile queries every fixed tick.
fn remember_sampled_silhouettes(
    commands: &mut Commands,
    shooter: Entity,
    delay: InterpolationDelay,
    targets: &Query<Entity, (With<PlayerMarker>, With<ControlledBy>)>,
    existing: &Query<(Entity, &LagCompensatedSilhouette)>,
    lag_compensation: &LagCompensationSpatialQuery,
) {
    for target in targets {
        if target == shooter {
            continue;
        }
        let Some(sample) = lag_compensation.sample_collider(delay, target) else {
            continue;
        };
        let silhouette = LagCompensatedSilhouette {
            shooter,
            target,
            position: sample.position.0,
            rotation: sample.rotation.as_radians(),
            sample_tick: sample.interpolation_tick,
            sample_overstep: sample.interpolation_overstep,
        };
        let timer = DespawnAfter(Timer::from_seconds(SILHOUETTE_LIFETIME, TimerMode::Once));

        if let Some((entity, _)) = existing
            .iter()
            .find(|(_, old)| old.shooter == shooter && old.target == target)
        {
            commands.entity(entity).insert((silhouette, timer));
        } else {
            commands.spawn((
                silhouette,
                timer,
                Name::new("Lag-compensated target silhouette"),
            ));
        }
    }
}

fn shooter_delay(
    shooter_id: PeerId,
    shooters: &Query<(Entity, &PlayerId, &ControlledBy), (With<PlayerMarker>, With<ControlledBy>)>,
    clients: &Query<&InterpolationDelay, With<ClientOf>>,
    host_clients: &Query<(), With<HostClient>>,
) -> Option<(Entity, InterpolationDelay)> {
    let (shooter, _, controlled_by) = shooters.iter().find(|(_, id, _)| id.0 == shooter_id)?;
    if host_clients.contains(controlled_by.owner) {
        Some((shooter, InterpolationDelay::default()))
    } else {
        clients
            .get(controlled_by.owner)
            .copied()
            .ok()
            .map(|delay| (shooter, delay))
    }
}

pub(crate) fn hitscan_hits(
    policy: Single<&HitPolicy, With<ClientContext>>,
    mut commands: Commands,
    hitscans: Query<
        (&hitscan::HitscanVisual, &BulletMarker),
        (Added<hitscan::HitscanVisual>, With<AuthoritativeProjectile>),
    >,
    targets: Query<Entity, (With<PlayerMarker>, With<ControlledBy>)>,
    shooters: Query<(Entity, &PlayerId, &ControlledBy), (With<PlayerMarker>, With<ControlledBy>)>,
    clients: Query<&InterpolationDelay, With<ClientOf>>,
    host_clients: Query<(), With<HostClient>>,
    lag_compensation: LagCompensationSpatialQuery,
    silhouettes: Query<(Entity, &LagCompensatedSilhouette)>,
    mut scores: Query<&mut Score, With<PlayerMarker>>,
) {
    if **policy != HitPolicy::ServerRewound {
        return;
    }

    for (shot, marker) in &hitscans {
        let Some((shooter, delay)) =
            shooter_delay(marker.shooter, &shooters, &clients, &host_clients)
        else {
            warn!(shooter = ?marker.shooter, "Cannot rewind hitscan without client timing metadata");
            continue;
        };
        remember_sampled_silhouettes(
            &mut commands,
            shooter,
            delay,
            &targets,
            &silhouettes,
            &lag_compensation,
        );
        let mut filter = SpatialQueryFilter::from_excluded_entities([shooter]);
        if let Some(result) = lag_compensation.cast_ray_predicate_with_sample(
            delay,
            shot.start,
            shot.direction(),
            hitscan::RANGE,
            true,
            &|entity| targets.contains(entity),
            &mut filter,
        ) {
            remember_hit(&mut commands, shot.start, shot.direction(), result);
            if let Ok(mut score) = scores.get_mut(shooter) {
                score.0 += 1;
            }
        }
    }
}

pub(crate) fn linear_hits(
    policy: Single<&HitPolicy, With<ClientContext>>,
    mut commands: Commands,
    projectiles: Query<
        (
            Entity,
            &Position,
            &linear::ProjectileSweepStart,
            &BulletMarker,
            Option<&BufferedProjectileOf>,
            Option<&BufferedSequence>,
        ),
        With<AuthoritativeProjectile>,
    >,
    targets: Query<Entity, (With<PlayerMarker>, With<ControlledBy>)>,
    shooters: Query<(Entity, &PlayerId, &ControlledBy), (With<PlayerMarker>, With<ControlledBy>)>,
    clients: Query<&InterpolationDelay, With<ClientOf>>,
    host_clients: Query<(), With<HostClient>>,
    lag_compensation: LagCompensationSpatialQuery,
    silhouettes: Query<(Entity, &LagCompensatedSilhouette)>,
    buffers: Query<&ShotBuffer>,
    mut scores: Query<&mut Score, With<PlayerMarker>>,
) {
    if **policy != HitPolicy::ServerRewound {
        return;
    }

    let mut sampled_shooters = HashSet::new();
    for (projectile, position, previous, marker, buffer_owner, sequence) in &projectiles {
        let segment = position.0 - previous.0;
        let distance = segment.length();
        let Some(direction) = segment.try_normalize() else {
            continue;
        };
        let Some((shooter, delay)) =
            shooter_delay(marker.shooter, &shooters, &clients, &host_clients)
        else {
            continue;
        };
        if sampled_shooters.insert(shooter) {
            remember_sampled_silhouettes(
                &mut commands,
                shooter,
                delay,
                &targets,
                &silhouettes,
                &lag_compensation,
            );
        }
        let mut filter = SpatialQueryFilter::from_excluded_entities([shooter]);
        if let Some(result) = lag_compensation.cast_ray_predicate_with_sample(
            delay,
            previous.0,
            Dir2::new_unchecked(direction),
            distance,
            true,
            &|entity| targets.contains(entity),
            &mut filter,
        ) {
            let direction = Dir2::new_unchecked(direction);
            remember_hit(&mut commands, previous.0, direction, result);
            if let (Some(owner), Some(sequence)) = (buffer_owner, sequence) {
                finish_linear_projectile(
                    &mut commands,
                    owner,
                    sequence,
                    &buffers,
                    lag_compensation.timeline.tick(),
                );
            }
            commands.entity(projectile).try_despawn();
            if let Ok(mut score) = scores.get_mut(shooter) {
                score.0 += 1;
            }
        }
    }
}
