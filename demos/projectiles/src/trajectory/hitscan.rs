//! Instantaneous hitscan trajectory.
//!
//! A hitscan evaluates a ray from its origin to its maximum range in one
//! simulation step. Any trail or flying tracer is presentation only; it does
//! not delay the authoritative hit. The 'projectile' is instant.

use avian2d::prelude::Rotation;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Authoritative hitscan range and the length of the rendered trail.
pub(crate) const RANGE: f32 = 2_000.0;

/// The local visual is deliberately short. The server copy lives longer so it
/// has time to be replicated to remote clients before it is despawned.
pub(crate) const LOCAL_VISUAL_LIFETIME: f32 = 0.15;
pub(crate) const SERVER_VISUAL_LIFETIME: f32 = 0.5;

/// Everything needed to draw and test a hitscan shot.
///
/// This component is also the state-entity representation for a hitscan. In
/// the fire-data representation it is placed on a local visual child instead.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub(crate) struct HitscanVisual {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
    pub(crate) lifetime: f32,
    pub(crate) max_lifetime: f32,
}

impl HitscanVisual {
    pub(crate) fn new(position: Vec2, rotation: Rotation, max_lifetime: f32) -> Self {
        let direction = rotation * Vec2::Y;
        Self {
            start: position,
            end: position + direction * RANGE,
            lifetime: 0.0,
            max_lifetime,
        }
    }

    pub(crate) fn direction(&self) -> Dir2 {
        // `new` always creates a non-zero segment because RANGE is positive.
        Dir2::new_unchecked((self.end - self.start).normalize())
    }
}

/// Advance the frame-time fade for every trail.
///
/// Projectile lifetime itself is fixed-tick based when a `DespawnAtTick`
/// component is present. A replicated state entity received by a remote client
/// has no local expiry component, so it retains this small presentation-only
/// fallback while waiting for the server's replicated despawn.
pub(crate) fn update_visuals(
    mut commands: Commands,
    time: Res<Time>,
    mut visuals: Query<(
        Entity,
        &mut HitscanVisual,
        Has<crate::shared::DespawnAtTick>,
    )>,
) {
    for (entity, mut visual, fixed_tick_expiry) in &mut visuals {
        visual.lifetime += time.delta_secs();
        if !fixed_tick_expiry && visual.lifetime >= visual.max_lifetime {
            commands.entity(entity).try_despawn();
        }
    }
}
