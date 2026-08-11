//! Constant-velocity linear projectile trajectory.
//!
//! A linear projectile derives its position from an initial position,
//! direction, speed, and elapsed fixed ticks. Collision must sweep from the
//! previous position to the next position rather than testing only the end.

use avian2d::prelude::*;
use bevy::prelude::*;

pub(crate) const SPEED: f32 = 300.0;
pub(crate) const LIFETIME_SECONDS: f32 = 3.0;

/// Start of the projectile segment that has not yet been checked for a hit.
///
/// For simulated projectiles this is captured before Avian advances
/// `Position`. For interpolated state projectiles the client advances it after
/// checking each newly sampled render position. It is local derived state and
/// never needs to cross the network.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct ProjectileSweepStart(pub(crate) Vec2);

pub(crate) fn velocity(rotation: Rotation, speed: f32) -> LinearVelocity {
    LinearVelocity(rotation * Vec2::Y * speed)
}

/// Capture the start of the segment before fixed-tick physics moves it.
pub(crate) fn capture_projectile_sweep_start(
    mut projectiles: Query<
        (&Position, &mut ProjectileSweepStart),
        With<crate::protocol::BulletMarker>,
    >,
) {
    for (position, mut previous) in &mut projectiles {
        previous.0 = position.0;
    }
}
