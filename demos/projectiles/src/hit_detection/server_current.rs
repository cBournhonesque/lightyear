//! Current-state server hit detection.
//!
//! The server evaluates the shot against colliders at its current authoritative
//! simulation time.
//!
//! # Advantages
//!
//! - Simple authority model with no history storage or rewind query.
//! - Resistant to clients inventing target poses or hit geometry.
//! - Useful as the negative control when demonstrating lag compensation.
//!
//! # Trade-offs
//!
//! - The shooter aims at an older interpolated target pose, while the server
//!   tests a newer pose, producing target advantage under latency.
//! - Players may need to lead targets in ways that feel inconsistent with the
//!   rendered view.
//! - Network latency directly changes practical weapon behavior.

use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::*;

use super::{AuthoritativeProjectile, accept_hit, impact_from_hit};
use crate::protocol::{Bot, BulletMarker, PlayerId, PlayerMarker, Score};
use crate::representation::shot_buffer::{
    BufferedProjectileOf, BufferedSequence, ShotBuffer, finish_linear_projectile,
};
use crate::trajectory::{hitscan, linear};

/// Test newly spawned authoritative hitscans against the current server world.
pub(crate) fn hitscan_hits(
    mut commands: Commands,
    hitscans: Query<
        (&hitscan::HitscanVisual, &BulletMarker),
        (Added<hitscan::HitscanVisual>, With<AuthoritativeProjectile>),
    >,
    targets: Query<(), (With<PlayerMarker>, With<ControlledBy>)>,
    players: Query<(Entity, &PlayerId), (With<PlayerMarker>, With<ControlledBy>)>,
    spatial_query: SpatialQuery,
    bots: Query<(), With<Bot>>,
    mut scores: Query<&mut Score, With<PlayerMarker>>,
) {
    for (shot, marker) in &hitscans {
        let Some(shooter) = players
            .iter()
            .find_map(|(entity, id)| (id.0 == marker.shooter).then_some(entity))
        else {
            continue;
        };
        let mut filter = SpatialQueryFilter::from_excluded_entities([shooter]);
        if let Some(hit) = spatial_query.cast_ray_predicate(
            shot.start,
            shot.direction(),
            hitscan::RANGE,
            true,
            &mut filter,
            &|entity| targets.contains(entity),
        ) {
            let impact = impact_from_hit(shot.start, shot.direction(), hit);
            accept_hit(
                &mut commands,
                shooter,
                hit.entity,
                impact,
                &bots,
                &mut scores,
            );
        }
    }
}

/// Sweep every authoritative linear projectile over the segment that physics
/// moved it this tick. This is the important fix for the old 0.5-unit ray,
/// which was much shorter than one tick of projectile movement.
pub(crate) fn linear_hits(
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
    targets: Query<(), (With<PlayerMarker>, With<ControlledBy>)>,
    players: Query<(Entity, &PlayerId), (With<PlayerMarker>, With<ControlledBy>)>,
    spatial_query: SpatialQuery,
    timeline: Res<LocalTimeline>,
    buffers: Query<&ShotBuffer>,
    bots: Query<(), With<Bot>>,
    mut scores: Query<&mut Score, With<PlayerMarker>>,
) {
    for (projectile, position, previous, marker, buffer_owner, sequence) in &projectiles {
        let Some(shooter) = players
            .iter()
            .find_map(|(entity, id)| (id.0 == marker.shooter).then_some(entity))
        else {
            continue;
        };
        let segment = position.0 - previous.0;
        let distance = segment.length();
        let Some(direction) = segment.try_normalize() else {
            continue;
        };
        let mut filter = SpatialQueryFilter::from_excluded_entities([shooter]);
        if let Some(hit) = spatial_query.cast_ray_predicate(
            previous.0,
            Dir2::new_unchecked(direction),
            distance,
            true,
            &mut filter,
            &|entity| targets.contains(entity),
        ) {
            let impact = impact_from_hit(previous.0, Dir2::new_unchecked(direction), hit);
            if let (Some(owner), Some(sequence)) = (buffer_owner, sequence) {
                finish_linear_projectile(&mut commands, owner, sequence, &buffers, timeline.tick());
            }
            commands.entity(projectile).try_despawn();
            accept_hit(
                &mut commands,
                shooter,
                hit.entity,
                impact,
                &bots,
                &mut scores,
            );
        }
    }
}
