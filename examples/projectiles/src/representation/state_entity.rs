//! Replicated state-entity representation.
//!
//! The server creates a network entity for each projectile and replicates its
//! changing state. The owner gets an immediate local copy in the predicted
//! timeline policies, while other clients receive predicted or interpolated
//! server entities according to the timeline axis. In predicted owner modes,
//! client and server use the same `PreSpawned` signature so the authoritative
//! entity confirms the already visible local projectile.
//!
//! # Advantages
//!
//! - Supports projectiles whose behavior changes after firing, including
//!   bounces, external forces, and dynamic homing.
//! - Gives each projectile normal entity identity, lifetime, interest, and
//!   component composition.
//! - Authoritative corrections are represented directly by state updates.
//!
//! # Trade-offs
//!
//! - Pays network entity spawn/despawn and entity-mapping costs per shot.
//! - Frequently changing position/velocity consumes more bandwidth.
//! - Prespawn signatures must be deterministic and unique for every projectile
//!   created by one shooter in the same tick.

use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::prelude::*;

use crate::hit_detection::AuthoritativeProjectile;
use crate::protocol::{BulletMarker, ColorComponent, PlayerId};
use crate::shared::{DespawnAtTick, ExpiryTimeline, ProjectileFireTick, expiry_tick};
use crate::timeline::TimelinePolicy;
use crate::trajectory::{TrajectoryKind, hitscan, linear};

/// Spawn one projectile whose gameplay state is the network representation.
///
/// This function deliberately contains the complete server/client branch. It
/// is a little repetitive, but it keeps the representation's behavior visible
/// without introducing a generic projectile factory.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shoot(
    commands: &mut Commands,
    prespawn_hash: u64,
    fire_tick: Tick,
    tick_duration: &TickDuration,
    position: Position,
    rotation: Rotation,
    player_id: PlayerId,
    color: ColorComponent,
    controlled_by: Option<&ControlledBy>,
    trajectory: TrajectoryKind,
    timeline: TimelinePolicy,
) {
    match trajectory {
        TrajectoryKind::Hitscan => {
            // In this representation the visible trace and the prespawn
            // candidate are the same entity. Keep that entity alive for the
            // server's replication window so normal latency or a retransmit
            // cannot remove the local matching candidate after only 150 ms.
            // The renderer still fades the line over LOCAL_VISUAL_LIFETIME.
            let lifetime = hitscan::SERVER_VISUAL_LIFETIME;
            let expiry = DespawnAtTick::new(
                expiry_tick(fire_tick, lifetime, tick_duration.0),
                ExpiryTimeline::Local,
            );
            let visual = (
                hitscan::HitscanVisual::new(position.0, rotation, lifetime),
                BulletMarker {
                    shooter: player_id.0,
                },
                player_id,
                color,
                ProjectileFireTick(fire_tick),
                Name::new("StateEntityHitscan"),
            );

            if let Some(controlled_by) = controlled_by {
                let mut entity = commands.spawn((
                    visual,
                    AuthoritativeProjectile,
                    controlled_by.clone(),
                    expiry,
                ));
                if timeline.owner_spawns_locally() {
                    entity.insert(PreSpawned::new(prespawn_hash).for_client(controlled_by.owner));
                }
                timeline.configure_projectile(&mut entity, player_id.0);
            } else if timeline.owner_spawns_locally() {
                commands.spawn((visual, expiry, PreSpawned::new(prespawn_hash)));
            }
        }
        TrajectoryKind::Linear => {
            let expiry = DespawnAtTick::new(
                expiry_tick(fire_tick, linear::LIFETIME_SECONDS, tick_duration.0),
                ExpiryTimeline::Local,
            );
            let projectile = (
                position,
                rotation,
                linear::velocity(rotation, linear::SPEED),
                linear::ProjectileSweepStart(position.0),
                RigidBody::Kinematic,
                BulletMarker {
                    shooter: player_id.0,
                },
                player_id,
                color,
                ProjectileFireTick(fire_tick),
                expiry,
                Name::new("StateEntityLinearProjectile"),
            );

            if let Some(controlled_by) = controlled_by {
                let mut entity =
                    commands.spawn((projectile, AuthoritativeProjectile, controlled_by.clone()));
                if timeline.owner_spawns_locally() {
                    entity.insert(PreSpawned::new(prespawn_hash).for_client(controlled_by.owner));
                }
                timeline.configure_projectile(&mut entity, player_id.0);
            } else if timeline.owner_spawns_locally() {
                commands.spawn((projectile, PreSpawned::new(prespawn_hash)));
            }
        }
    }
}
