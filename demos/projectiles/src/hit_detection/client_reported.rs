//! Client-reported hit detection.
//!
//! The firing client tests the colliders it rendered and reports its claimed
//! target to the server. This is intentionally an insecure comparison mode,
//! not a recommended authority model.
//!
//! # Advantages
//!
//! - The hit naturally agrees with what the shooter saw.
//! - Requires no historical collision query on the server.
//! - Useful for teaching why responsiveness and trust are separate concerns.
//!
//! # Trade-offs
//!
//! - A malicious client can invent hit geometry unless the server performs
//!   equivalent validation, which would remove the main simplicity benefit.
//! - The server should still validate sender ownership, cadence, target
//!   existence, duplicate/replay handling, and shot identity.
//! - Other peers may observe outcomes that do not agree with their timelines.

use avian2d::prelude::*;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use lightyear::prelude::*;

use super::remember_impact;
use crate::protocol::{BulletMarker, HitChannel, HitDetected, PlayerId, PlayerMarker};
use crate::shared::{PLAYER_SIZE, ProjectileFireTick};
use crate::trajectory::{hitscan, linear};

/// Local rollback-resistant record of client-reported shots.
///
/// Rollback can recreate an `Added<HitscanVisual>` or a linear projectile for
/// the same fire tick. The server intentionally trusts this mode, so emitting
/// that report again would also award the score again.
#[derive(Resource, Default)]
pub(crate) struct ReportedClientHits(HashSet<(PeerId, Tick)>);

impl ReportedClientHits {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    fn report_once(&mut self, shooter: PeerId, fire_tick: Tick) -> bool {
        self.0.insert((shooter, fire_tick))
    }
}

type RenderedPlayerFilter = (
    With<PlayerMarker>,
    Without<ControlledBy>,
    Or<(With<Predicted>, With<Interpolated>)>,
);

/// Raycast directly against the player poses rendered by this client.
///
/// Client-reported mode previously installed Avian colliders on predicted and
/// interpolated replicas. Arena resets could then roll the collider-tree
/// resource behind those newly created collider proxy keys, causing Avian's
/// `StableVec` panic. The example only has one simple rectangular player
/// shape, so querying that shape directly is both clearer and independent of
/// the client's rollback physics world.
fn closest_rendered_player_hit(
    targets: &Query<(Entity, &Position, &Rotation), RenderedPlayerFilter>,
    shooter: Entity,
    origin: Vec2,
    direction: Dir2,
    max_distance: f32,
) -> Option<RayHitData> {
    let player_shape = Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE);
    let mut closest: Option<RayHitData> = None;

    for (entity, position, rotation) in targets {
        if entity == shooter {
            continue;
        }
        let Some((distance, normal)) = player_shape.cast_ray(
            *position,
            *rotation,
            origin,
            direction.as_vec2(),
            max_distance,
            true,
        ) else {
            continue;
        };
        if closest.is_none_or(|hit| distance < hit.distance) {
            closest = Some(RayHitData {
                entity,
                distance,
                normal,
            });
        }
    }

    closest
}

pub(crate) fn hitscan_hits(
    mut commands: Commands,
    hitscans: Query<
        (
            &hitscan::HitscanVisual,
            &BulletMarker,
            &PlayerId,
            &ProjectileFireTick,
        ),
        Added<hitscan::HitscanVisual>,
    >,
    targets: Query<(Entity, &Position, &Rotation), RenderedPlayerFilter>,
    players: Query<
        (Entity, &PlayerId, Has<Controlled>),
        (With<PlayerMarker>, Without<ControlledBy>),
    >,
    mut reported: ResMut<ReportedClientHits>,
    mut sender: Single<(&LocalId, &mut EventSender<HitDetected>), With<Client>>,
) {
    let (local_id, sender) = &mut *sender;
    for (shot, marker, player_id, fire_tick) in &hitscans {
        // A client reports only its own shots. Remote visuals still run through
        // this system, but are presentation and must never create hit events.
        if player_id.0 != local_id.0 {
            continue;
        }
        let Some(shooter) = players.iter().find_map(|(entity, id, controlled)| {
            (controlled && id.0 == marker.shooter).then_some(entity)
        }) else {
            continue;
        };
        if let Some(hit) = closest_rendered_player_hit(
            &targets,
            shooter,
            shot.start,
            shot.direction(),
            hitscan::RANGE,
        ) {
            if !reported.report_once(marker.shooter, fire_tick.0) {
                continue;
            }
            debug!(
                ?shooter,
                target = ?hit.entity,
                distance = hit.distance,
                "Client reporting hitscan hit"
            );
            let impact = remember_impact(&mut commands, shot.start, shot.direction(), hit);
            sender.trigger::<HitChannel>(HitDetected {
                shooter,
                target: hit.entity,
                impact,
            });
        }
    }
}

pub(crate) fn linear_hits(
    mut commands: Commands,
    mut projectiles: Query<(
        Entity,
        &Position,
        &mut linear::ProjectileSweepStart,
        &BulletMarker,
        &PlayerId,
        &ProjectileFireTick,
    )>,
    targets: Query<(Entity, &Position, &Rotation), RenderedPlayerFilter>,
    players: Query<
        (Entity, &PlayerId, Has<Controlled>),
        (With<PlayerMarker>, Without<ControlledBy>),
    >,
    mut reported: ResMut<ReportedClientHits>,
    mut sender: Single<(&LocalId, &mut EventSender<HitDetected>), With<Client>>,
) {
    let (local_id, sender) = &mut *sender;
    for (projectile, position, mut sweep_start, marker, player_id, fire_tick) in &mut projectiles {
        // Interpolated state entities move when Lightyear samples them in
        // `Update`. Remember the point checked by this invocation so the next
        // query covers exactly the newly rendered segment.
        let previous_position = sweep_start.0;
        sweep_start.0 = position.0;
        if player_id.0 != local_id.0 {
            continue;
        }
        let Some(shooter) = players.iter().find_map(|(entity, id, controlled)| {
            (controlled && id.0 == marker.shooter).then_some(entity)
        }) else {
            continue;
        };
        let segment = position.0 - previous_position;
        let distance = segment.length();
        let Some(direction) = segment.try_normalize() else {
            continue;
        };
        if let Some(hit) = closest_rendered_player_hit(
            &targets,
            shooter,
            previous_position,
            Dir2::new_unchecked(direction),
            distance,
        ) {
            if reported.report_once(marker.shooter, fire_tick.0) {
                debug!(
                    ?shooter,
                    target = ?hit.entity,
                    distance = hit.distance,
                    "Client reporting linear-projectile hit"
                );
                let impact = remember_impact(
                    &mut commands,
                    previous_position,
                    Dir2::new_unchecked(direction),
                    hit,
                );
                sender.trigger::<HitChannel>(HitDetected {
                    shooter,
                    target: hit.entity,
                    impact,
                });
            }
            commands.entity(projectile).try_despawn();
        }
    }
}
