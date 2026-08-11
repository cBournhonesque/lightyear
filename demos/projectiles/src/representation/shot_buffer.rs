//! Replicated shot-buffer representation.
//!
//! The player owns a small fixed-capacity ring of recent fire records. Firing
//! changes one slot and advances a monotonic sequence; each peer keeps a local
//! cursor and materializes newly visible records as ordinary local entities.
//! There is no replicated entity per projectile.
//!
//! # Advantages
//!
//! - Avoids replicated entity spawn/despawn overhead for frequent,
//!   short-lived projectiles.
//! - Replicon diffs send one changed slot instead of the complete buffer.
//! - The owner predicts the same buffer write as the server, so its projectile
//!   appears immediately without prespawn matching.
//!
//! # Trade-offs
//!
//! - The ring needs a sequence, local consumer cursor, wrap/overrun handling,
//!   and explicit finish updates for moving projectiles.
//! - Shots inherit the replication visibility and lifetime of their player;
//!   they cannot remain network-relevant after that player disappears.
//! - The compact records only support trajectories that can be reconstructed
//!   deterministically. Complex mutable physics belongs in a state entity.

use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable as RepliconDiffable, EntityCommandsDiffExt};
use lightyear::prediction::rollback::DisableRollback;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::hit_detection::AuthoritativeProjectile;
use crate::protocol::{BulletMarker, ColorComponent, PlayerId, PlayerMarker};
use crate::shared::{DespawnAtTick, ExpiryTimeline, ProjectileFireTick, expiry_tick};
use crate::trajectory::{TrajectoryKind, hitscan, linear};

/// Large enough for more than one full linear-projectile lifetime at the
/// example's current fire rates. Materialized local entities can outlive the
/// slot; the capacity only bounds how much unconsumed fire history is retained.
pub(crate) const SHOT_BUFFER_CAPACITY: usize = 32;

/// Sparse, deterministic facts needed to reconstruct one shot.
///
/// Shooter identity and color are intentionally omitted because the record is
/// stored on that player's entity. `finish_tick` changes at most once, when an
/// authoritative linear projectile hits something.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct ShotRecord {
    pub(crate) sequence: u64,
    pub(crate) fire_tick: Tick,
    pub(crate) position: Position,
    pub(crate) rotation: Rotation,
    pub(crate) trajectory: TrajectoryKind,
    pub(crate) finish_tick: Option<Tick>,
}

/// Fixed networked array plus the next sequence to allocate.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct ShotBuffer {
    pub(crate) next_sequence: u64,
    slots: [Option<ShotRecord>; SHOT_BUFFER_CAPACITY],
}

impl Default for ShotBuffer {
    fn default() -> Self {
        Self {
            next_sequence: 0,
            slots: core::array::from_fn(|_| None),
        }
    }
}

impl ShotBuffer {
    fn oldest_retained_sequence(&self) -> u64 {
        self.next_sequence
            .saturating_sub(SHOT_BUFFER_CAPACITY as u64)
    }

    fn record(&self, sequence: u64) -> Option<&ShotRecord> {
        self.slots[sequence as usize % SHOT_BUFFER_CAPACITY]
            .as_ref()
            .filter(|record| record.sequence == sequence)
    }
}

/// Small updates recorded by Replicon instead of retransmitting all 32 slots.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) enum ShotBufferDiff {
    Fire(ShotRecord),
    Finish { sequence: u64, finish_tick: Tick },
}

impl RepliconDiffable for ShotBuffer {
    type Diff = ShotBufferDiff;

    // This is replication recovery history, separate from the 32 retained
    // gameplay records. It comfortably covers bursts between full snapshots.
    const HISTORY_LEN: usize = 128;

    fn apply_diff(&mut self, diff: &Self::Diff) -> bevy::ecs::error::Result<()> {
        match diff {
            ShotBufferDiff::Fire(record) => {
                let slot = record.sequence as usize % SHOT_BUFFER_CAPACITY;
                self.slots[slot] = Some(record.clone());
                self.next_sequence = self.next_sequence.max(record.sequence.saturating_add(1));
            }
            ShotBufferDiff::Finish {
                sequence,
                finish_tick,
            } => {
                let slot = *sequence as usize % SHOT_BUFFER_CAPACITY;
                if let Some(record) = self.slots[slot]
                    .as_mut()
                    .filter(|record| record.sequence == *sequence)
                {
                    // Receiving the same finish twice is harmless. Keeping the
                    // earliest tick also makes a corrected result conservative.
                    record.finish_tick = Some(
                        record
                            .finish_tick
                            .map_or(*finish_tick, |old| old.min(*finish_tick)),
                    );
                }
            }
        }
        Ok(())
    }
}

/// A shot stream is discrete state, not a numeric value. The end snapshot may
/// be exposed early because `materialize` still waits for each record's exact
/// fire tick on the interpolation timeline.
pub(crate) fn interpolate_shot_buffer(_start: ShotBuffer, end: ShotBuffer, _t: f32) -> ShotBuffer {
    end
}

/// Local relationship from a materialized projectile to its player/buffer.
#[derive(Component, Debug)]
#[relationship(relationship_target = BufferedProjectiles)]
pub(crate) struct BufferedProjectileOf(pub(crate) Entity);

/// All local projectile entities materialized from one player's buffer.
/// `linked_spawn` ensures an arena reset or player despawn removes them too.
#[derive(Component, Debug)]
#[relationship_target(relationship = BufferedProjectileOf, linked_spawn)]
pub(crate) struct BufferedProjectiles(Vec<Entity>);

/// Local bookkeeping for a materialized record. This is not replicated shot
/// identity and does not create an extra entity; it only lets the consumer
/// reconcile a ring slot with the local projectile it already created.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BufferedSequence(pub(crate) u64);

/// Fire geometry used to create a local projectile.
///
/// Prediction can correct the buffered origin or direction while retaining the
/// same sequence. Remembering the consumed geometry lets us replace that local
/// visual instead of mistaking it for an already-correct duplicate.
#[derive(Component, Clone, Debug, PartialEq)]
pub(crate) struct MaterializedShot {
    fire_tick: Tick,
    position: Position,
    rotation: Rotation,
    trajectory: TrajectoryKind,
}

impl From<&ShotRecord> for MaterializedShot {
    fn from(record: &ShotRecord) -> Self {
        Self {
            fire_tick: record.fire_tick,
            position: record.position,
            rotation: record.rotation,
            trajectory: record.trajectory,
        }
    }
}

/// Next sequence this app should materialize for one player.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct ShotBufferCursor {
    next_sequence: u64,
}

/// Append the locally predicted or server-authoritative fire record.
pub(crate) fn shoot(
    commands: &mut Commands,
    player: Entity,
    buffer: &ShotBuffer,
    fire_tick: Tick,
    position: Position,
    rotation: Rotation,
    trajectory: TrajectoryKind,
) {
    let record = ShotRecord {
        sequence: buffer.next_sequence,
        fire_tick,
        position,
        rotation,
        trajectory,
        finish_tick: None,
    };
    debug!(
        ?player,
        sequence = record.sequence,
        ?fire_tick,
        trajectory = trajectory.name(),
        "Appending projectile to shot buffer"
    );
    commands
        .entity(player)
        .apply_diff::<ShotBuffer>(ShotBufferDiff::Fire(record));
}

/// Consume newly visible records and create local simulation/presentation.
///
/// An interpolated player waits until its interpolation timeline reaches the
/// record's fire tick. Late records are caught up analytically, and records
/// that have already expired are simply consumed without spawning a visual.
#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize(
    mut commands: Commands,
    local_timeline: Res<LocalTimeline>,
    interpolation_timeline: Option<Res<InterpolationTimeline>>,
    tick_duration: Res<lightyear::core::tick::TickDuration>,
    mut players: Query<
        (
            Entity,
            &ShotBuffer,
            &PlayerId,
            &ColorComponent,
            Has<Interpolated>,
            Has<ControlledBy>,
            Option<&mut ShotBufferCursor>,
        ),
        With<PlayerMarker>,
    >,
    existing: Query<(&BufferedProjectileOf, &BufferedSequence)>,
) {
    let interpolation_tick = interpolation_timeline
        .filter(|timeline| timeline.is_synced())
        .map(|timeline| timeline.tick());

    for (player, buffer, player_id, color, interpolated, authoritative, cursor) in &mut players {
        // In an all-features host-client app an authoritative server entity can
        // also carry client presentation markers. Server authority wins: hit
        // detection and lifetime must always use the local simulation tick.
        let uses_interpolation = interpolated && !authoritative;
        let presentation_tick = if uses_interpolation {
            let Some(tick) = interpolation_tick else {
                continue;
            };
            tick
        } else {
            local_timeline.tick()
        };
        let expiry_timeline = if uses_interpolation {
            ExpiryTimeline::Interpolation
        } else {
            ExpiryTimeline::Local
        };

        let oldest = buffer.oldest_retained_sequence();
        let mut next_sequence = cursor
            .as_ref()
            .map_or(oldest, |cursor| cursor.next_sequence);
        if next_sequence < oldest {
            warn!(
                ?player,
                next_sequence,
                oldest,
                head = buffer.next_sequence,
                "Shot-buffer consumer overrun; skipping overwritten records"
            );
            next_sequence = oldest;
        } else if next_sequence > buffer.next_sequence {
            // Prediction rollback can temporarily move the authoritative head
            // backwards. Resume at the oldest record that still exists.
            next_sequence = oldest;
        }

        while next_sequence < buffer.next_sequence {
            let Some(record) = buffer.record(next_sequence) else {
                next_sequence += 1;
                continue;
            };
            if presentation_tick < record.fire_tick {
                break;
            }

            let already_exists = existing
                .iter()
                .any(|(owner, sequence)| owner.0 == player && sequence.0 == record.sequence);
            if !already_exists {
                spawn_record(
                    &mut commands,
                    player,
                    record,
                    *player_id,
                    *color,
                    presentation_tick,
                    expiry_timeline,
                    authoritative,
                    tick_duration.0,
                );
            }
            next_sequence += 1;
        }

        if let Some(mut cursor) = cursor {
            cursor.next_sequence = next_sequence;
        } else {
            commands
                .entity(player)
                .insert(ShotBufferCursor { next_sequence });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_record(
    commands: &mut Commands,
    player: Entity,
    record: &ShotRecord,
    player_id: PlayerId,
    color: ColorComponent,
    presentation_tick: Tick,
    expiry_timeline: ExpiryTimeline,
    authoritative: bool,
    tick_duration: core::time::Duration,
) {
    let elapsed_ticks = (presentation_tick - record.fire_tick).max(0) as f32;
    let elapsed_seconds = elapsed_ticks * tick_duration.as_secs_f32();
    debug!(
        ?player,
        sequence = record.sequence,
        trajectory = record.trajectory.name(),
        ?presentation_tick,
        ?expiry_timeline,
        authoritative,
        elapsed_seconds,
        "Consuming buffered projectile record"
    );

    match record.trajectory {
        TrajectoryKind::Hitscan => {
            let lifetime = if authoritative {
                hitscan::SERVER_VISUAL_LIFETIME
            } else {
                hitscan::LOCAL_VISUAL_LIFETIME
            };
            if elapsed_seconds >= lifetime {
                return;
            }

            let mut visual =
                hitscan::HitscanVisual::new(record.position.0, record.rotation, lifetime);
            visual.lifetime = elapsed_seconds;
            let mut entity = commands.spawn((
                visual,
                BulletMarker {
                    shooter: player_id.0,
                },
                player_id,
                color,
                ProjectileFireTick(record.fire_tick),
                DespawnAtTick::new(
                    expiry_tick(record.fire_tick, lifetime, tick_duration),
                    expiry_timeline,
                ),
                BufferedProjectileOf(player),
                BufferedSequence(record.sequence),
                MaterializedShot::from(record),
                DisableRollback,
                Name::new("ShotBufferHitscanVisual"),
            ));
            if authoritative {
                entity.insert(AuthoritativeProjectile);
            }
        }
        TrajectoryKind::Linear => {
            let natural_expiry =
                expiry_tick(record.fire_tick, linear::LIFETIME_SECONDS, tick_duration);
            let final_expiry = record
                .finish_tick
                .map_or(natural_expiry, |finish| natural_expiry.min(finish));
            if presentation_tick >= final_expiry {
                return;
            }

            let velocity = linear::velocity(record.rotation, linear::SPEED);
            let position = Position(record.position.0 + velocity.0 * elapsed_seconds);
            let mut entity = commands.spawn((
                position,
                record.rotation,
                velocity,
                linear::ProjectileSweepStart(position.0),
                RigidBody::Kinematic,
                BulletMarker {
                    shooter: player_id.0,
                },
                player_id,
                color,
                ProjectileFireTick(record.fire_tick),
                DespawnAtTick::new(final_expiry, expiry_timeline),
                BufferedProjectileOf(player),
                BufferedSequence(record.sequence),
                MaterializedShot::from(record),
                DisableRollback,
                Name::new("ShotBufferLinearProjectile"),
            ));
            if authoritative {
                entity.insert(AuthoritativeProjectile);
            }
        }
    }
}

/// Reconcile local presentation with corrected predicted buffer geometry.
///
/// A visual may safely outlive an overwritten ring slot. We only remove it if
/// its sequence should still be retained but is absent, or if prediction moved
/// the buffer head behind it. If the same sequence now contains corrected fire
/// data, replace the local entity at the corrected analytic position.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile_materialized(
    mut commands: Commands,
    local_timeline: Res<LocalTimeline>,
    interpolation_timeline: Option<Res<InterpolationTimeline>>,
    tick_duration: Res<lightyear::core::tick::TickDuration>,
    players: Query<(&ShotBuffer, &PlayerId, &ColorComponent)>,
    projectiles: Query<(
        Entity,
        &BufferedProjectileOf,
        &BufferedSequence,
        &MaterializedShot,
        &DespawnAtTick,
        Has<AuthoritativeProjectile>,
    )>,
) {
    let interpolation_tick = interpolation_timeline
        .filter(|timeline| timeline.is_synced())
        .map(|timeline| timeline.tick());

    for (entity, owner, sequence, materialized, expiry, authoritative) in &projectiles {
        let Ok((buffer, player_id, color)) = players.get(owner.0) else {
            commands.entity(entity).try_despawn();
            continue;
        };
        if sequence.0 >= buffer.next_sequence {
            commands.entity(entity).try_despawn();
            continue;
        }
        let oldest = buffer.oldest_retained_sequence();
        let Some(record) = buffer.record(sequence.0) else {
            // Older visuals are allowed to outlive the slot that created them.
            // A record inside the retained range should never be absent unless
            // prediction was rejected or corrected.
            if sequence.0 >= oldest {
                commands.entity(entity).try_despawn();
            }
            continue;
        };
        if *materialized == MaterializedShot::from(record) {
            continue;
        }

        let presentation_tick = match expiry.timeline {
            ExpiryTimeline::Local => local_timeline.tick(),
            ExpiryTimeline::Interpolation => {
                let Some(tick) = interpolation_tick else {
                    continue;
                };
                tick
            }
        };

        commands.entity(entity).try_despawn();
        spawn_record(
            &mut commands,
            owner.0,
            record,
            *player_id,
            *color,
            presentation_tick,
            expiry.timeline,
            authoritative,
            tick_duration.0,
        );
    }
}

/// Record an authoritative linear impact so clients can stop their local
/// reconstruction without needing a per-projectile despawn message.
pub(crate) fn finish_linear_projectile(
    commands: &mut Commands,
    owner: &BufferedProjectileOf,
    sequence: &BufferedSequence,
    buffers: &Query<&ShotBuffer>,
    finish_tick: Tick,
) {
    let Ok(buffer) = buffers.get(owner.0) else {
        return;
    };
    if buffer
        .record(sequence.0)
        .is_some_and(|record| record.finish_tick.is_none())
    {
        commands
            .entity(owner.0)
            .apply_diff::<ShotBuffer>(ShotBufferDiff::Finish {
                sequence: sequence.0,
                finish_tick,
            });
    }
}

/// Apply a finish update to an already materialized local projectile. The
/// common fixed-tick expiry system performs the actual despawn on the correct
/// local or interpolation timeline.
pub(crate) fn apply_authoritative_finishes(
    buffers: Query<&ShotBuffer>,
    mut projectiles: Query<(&BufferedProjectileOf, &BufferedSequence, &mut DespawnAtTick)>,
) {
    for (owner, sequence, mut expiry) in &mut projectiles {
        let Some(finish_tick) = buffers
            .get(owner.0)
            .ok()
            .and_then(|buffer| buffer.record(sequence.0))
            .and_then(|record| record.finish_tick)
        else {
            continue;
        };
        expiry.tick = expiry.tick.min(finish_tick);
    }
}
