//! Constant-velocity linear projectile trajectory.
//!
//! A linear projectile derives its position from an initial position,
//! direction, speed, and elapsed fixed ticks. Collision must sweep from the
//! previous position to the next position rather than testing only the end.
//!
//! # Advantages
//!
//! - Cheap and deterministic enough to reconstruct from immutable fire data.
//! - Naturally demonstrates projectile travel time and dodging.
//! - The same trajectory function can drive state entities, fire-data
//!   entities, and shot-buffer visuals.
//!
//! # Trade-offs
//!
//! - Requires fixed-tick lifetime and swept collision to prevent tunneling.
//! - Prediction errors in origin or direction remain visible for the whole
//!   flight unless the authoritative shot is reconciled.
//! - It cannot represent arbitrary bounces, forces, or changing homing targets
//!   without adding state or corrections.

use avian2d::prelude::*;
use bevy::prelude::*;

pub(crate) const SPEED: f32 = 300.0;
pub(crate) const LIFETIME_SECONDS: f32 = 3.0;

/// Position at the beginning of the current fixed tick.
///
/// Avian advances `Position` during physics. Keeping the previous value lets
/// hit detection sweep the exact segment the projectile traversed instead of
/// using the old unreliable fixed 0.5-unit ray.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct PreviousProjectilePosition(pub(crate) Vec2);

pub(crate) fn velocity(rotation: Rotation, speed: f32) -> LinearVelocity {
    LinearVelocity(rotation * Vec2::Y * speed)
}

/// Capture each projectile's start-of-tick position before physics moves it.
pub(crate) fn remember_previous_position(
    mut projectiles: Query<
        (&Position, &mut PreviousProjectilePosition),
        With<crate::protocol::BulletMarker>,
    >,
) {
    for (position, mut previous) in &mut projectiles {
        previous.0 = position.0;
    }
}
