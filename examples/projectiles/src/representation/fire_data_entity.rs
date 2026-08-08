//! Replicated fire-data entity representation.
//!
//! The server creates one network entity per shot but replicates only its
//! initial, immutable trajectory data. Each peer creates a local projectile
//! visual and reconstructs its current position from the fire tick.
//!
//! # Advantages
//!
//! - Saves bandwidth by avoiding continuous position and velocity updates.
//! - Retains a normal per-projectile network identity for lifetime and
//!   interest management.
//! - Late receivers can reconstruct an active projectile from its fire tick
//!   instead of displaying it from the muzzle with an extra packet delay.
//!
//! # Trade-offs
//!
//! - Still pays network entity insertion, removal, and mapping costs per shot.
//! - Only works without corrections when the trajectory can be reproduced
//!   from the replicated data. The active moving implementation is linear;
//!   arbitrary physics, bounces, and mutable homing need state updates.
//! - The local visual is deliberately not replicated, so hit authority must
//!   live elsewhere and the parent entity owns its network lifetime.

use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::prediction::rollback::DisableRollback;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::hit_detection::AuthoritativeProjectile;
use crate::protocol::{BulletMarker, ColorComponent, PlayerId};
use crate::shared::{DespawnAtTick, ExpiryTimeline, ProjectileFireTick, expiry_tick};
use crate::timeline::TimelinePolicy;
use crate::trajectory::{TrajectoryKind, hitscan, linear};

/// Immutable facts needed to reconstruct a shot on any peer.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub(crate) struct FireData {
    pub(crate) fire_tick: Tick,
    pub(crate) position: Position,
    pub(crate) rotation: Rotation,
    pub(crate) speed: f32,
    pub(crate) color: ColorComponent,
    pub(crate) trajectory: TrajectoryKind,
    pub(crate) player_id: PeerId,
}

/// Identifies the fire-data entity that owns a local projectile visual.
#[derive(Component, Debug)]
#[relationship(relationship_target = ProjectileVisuals)]
pub(crate) struct ProjectileVisualOf(pub(crate) Entity);

/// Local visuals created from one fire-data entity.
#[derive(Component, Debug)]
#[relationship_target(relationship = ProjectileVisualOf, linked_spawn)]
pub(crate) struct ProjectileVisuals(Vec<Entity>);

/// Local-only marker. It lets materialization distinguish the server's
/// authoritative simulation from client presentation without consulting the
/// application's feature flags.
#[derive(Component)]
pub(crate) struct AuthoritativeFireData;

#[allow(clippy::too_many_arguments)]
pub(crate) fn shoot(
    commands: &mut Commands,
    prespawn_hash: u64,
    fire_tick: Tick,
    position: Position,
    rotation: Rotation,
    player_id: PlayerId,
    color: ColorComponent,
    controlled_by: Option<&ControlledBy>,
    trajectory: TrajectoryKind,
    timeline: TimelinePolicy,
) {
    let speed = match trajectory {
        TrajectoryKind::Hitscan => 0.0,
        TrajectoryKind::Linear => linear::SPEED,
    };
    let fire_data = FireData {
        fire_tick,
        position,
        rotation,
        speed,
        color,
        trajectory,
        player_id: player_id.0,
    };

    if let Some(controlled_by) = controlled_by {
        let mut entity = commands.spawn((
            fire_data,
            AuthoritativeFireData,
            controlled_by.clone(),
            Name::new("FireDataEntity"),
        ));
        if timeline.owner_spawns_locally() {
            entity.insert(PreSpawned::new(prespawn_hash).for_client(controlled_by.owner));
        }
        timeline.configure_projectile(&mut entity, player_id.0);
    } else if timeline.owner_spawns_locally() {
        // The authoritative FireData entity will be mapped onto this same
        // local parent. Its non-networked visual child survives the match.
        commands.spawn((
            fire_data,
            PreSpawned::new(prespawn_hash),
            Name::new("LocalFireDataEntity"),
        ));
    }
}

/// Turn newly available fire data into a local simulation/presentation entity.
///
/// Interpolated fire data waits until the interpolation timeline reaches the
/// fire tick. If it arrives late, linear projectiles are advanced to the
/// correct position and expired effects are skipped.
pub(crate) fn materialize(
    mut commands: Commands,
    local_timeline: Res<LocalTimeline>,
    interpolation_timeline: Option<Res<InterpolationTimeline>>,
    tick_duration: Res<lightyear::core::tick::TickDuration>,
    fire_data: Query<
        (
            Entity,
            &FireData,
            Has<Interpolated>,
            Has<AuthoritativeFireData>,
        ),
        Without<ProjectileVisuals>,
    >,
) {
    let interpolation_tick = interpolation_timeline
        .filter(|timeline| timeline.is_synced())
        .map(|timeline| timeline.tick());

    for (fire_entity, fire, interpolated, authoritative) in &fire_data {
        // Host-client server entities may also have presentation markers. The
        // authoritative simulation must nevertheless use the local tick.
        let uses_interpolation = interpolated && !authoritative;
        let presentation_tick = if uses_interpolation {
            let Some(interpolation_tick) = interpolation_tick else {
                continue;
            };
            if interpolation_tick < fire.fire_tick {
                continue;
            }
            interpolation_tick
        } else {
            local_timeline.tick()
        };
        let expiry_timeline = if uses_interpolation {
            ExpiryTimeline::Interpolation
        } else {
            ExpiryTimeline::Local
        };

        let elapsed_ticks = (presentation_tick - fire.fire_tick).max(0) as f32;
        let elapsed_seconds = elapsed_ticks * tick_duration.0.as_secs_f32();

        match fire.trajectory {
            TrajectoryKind::Hitscan => {
                let max_lifetime = if authoritative {
                    hitscan::SERVER_VISUAL_LIFETIME
                } else {
                    hitscan::LOCAL_VISUAL_LIFETIME
                };
                if elapsed_seconds >= max_lifetime {
                    commands.entity(fire_entity).try_despawn();
                    continue;
                }

                let mut visual =
                    hitscan::HitscanVisual::new(fire.position.0, fire.rotation, max_lifetime);
                visual.lifetime = elapsed_seconds;
                let mut entity = commands.spawn((
                    visual,
                    BulletMarker {
                        shooter: fire.player_id,
                    },
                    PlayerId(fire.player_id),
                    fire.color,
                    ProjectileFireTick(fire.fire_tick),
                    DespawnAtTick::new(
                        expiry_tick(fire.fire_tick, max_lifetime, tick_duration.0),
                        expiry_timeline,
                    ),
                    ProjectileVisualOf(fire_entity),
                    DisableRollback,
                    Name::new("FireDataHitscanVisual"),
                ));
                if authoritative {
                    entity.insert(AuthoritativeProjectile);
                }
            }
            TrajectoryKind::Linear => {
                if elapsed_seconds >= linear::LIFETIME_SECONDS {
                    commands.entity(fire_entity).try_despawn();
                    continue;
                }

                let velocity = linear::velocity(fire.rotation, fire.speed);
                let position = Position(fire.position.0 + velocity.0 * elapsed_seconds);
                let mut entity = commands.spawn((
                    position,
                    fire.rotation,
                    velocity,
                    linear::PreviousProjectilePosition(position.0),
                    RigidBody::Kinematic,
                    PlayerId(fire.player_id),
                    fire.color,
                    ProjectileFireTick(fire.fire_tick),
                    BulletMarker {
                        shooter: fire.player_id,
                    },
                    DespawnAtTick::new(
                        expiry_tick(fire.fire_tick, linear::LIFETIME_SECONDS, tick_duration.0),
                        expiry_timeline,
                    ),
                    ProjectileVisualOf(fire_entity),
                    DisableRollback,
                    Name::new("FireDataLinearProjectile"),
                ));
                if authoritative {
                    entity.insert(AuthoritativeProjectile);
                }
            }
        }
    }
}

/// When the local visual expires or hits something, remove its fire-data
/// parent. On the server that despawns the replicated network entity too.
pub(crate) fn despawn_parent(
    trigger: On<Remove, ProjectileVisualOf>,
    visuals: Query<&ProjectileVisualOf, With<BulletMarker>>,
    authoritative_parents: Query<(), With<AuthoritativeFireData>>,
    mut commands: Commands,
) {
    if let Ok(parent) = visuals.get(trigger.entity)
        && authoritative_parents.contains(parent.0)
        && let Ok(mut entity) = commands.get_entity(parent.0)
    {
        entity.try_despawn();
    }
}
