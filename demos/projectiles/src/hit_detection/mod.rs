//! Projectile hit-policy axis.
//!
//! This axis describes which peer decides a hit and which point in world
//! history it queries. The selected policy consumes the same projectile
//! markers regardless of which network representation created them.
//!
//! The server-owned `HitPolicy` component selects exactly one implementation.

use avian2d::prelude::RayHitData;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::protocol::ClientContext;
use crate::shared::DespawnAfter;

pub(crate) mod client_reported;
pub(crate) mod server_current;
pub(crate) mod server_rewound;

/// Selects the authority and world time used for projectile hit detection.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect, Serialize, Deserialize,
)]
pub(crate) enum HitPolicy {
    /// The server queries its current authoritative world.
    #[default]
    ServerCurrent,
    /// The server queries retained collider history at the shooter's view time.
    ServerRewound,
    /// The server deliberately trusts client-reported hit geometry.
    ClientReported,
}

impl HitPolicy {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::ServerCurrent => Self::ServerRewound,
            Self::ServerRewound => Self::ClientReported,
            Self::ClientReported => Self::ServerCurrent,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ServerCurrent => "Server current state",
            Self::ServerRewound => "Server lag compensated",
            Self::ClientReported => "Client reported (insecure)",
        }
    }
}

/// Run a hit-detection implementation only while its policy is selected.
///
/// Keeping policy selection in the schedule makes it immediately visible at
/// each system's registration site and avoids fetching all of an inactive
/// implementation's system parameters every tick. The query form also makes
/// the condition simply return `false` while the replicated global context is
/// not available yet.
pub(crate) fn hit_policy_is(
    expected: HitPolicy,
) -> impl Fn(Query<&HitPolicy, With<ClientContext>>) -> bool + Clone {
    move |policies| policies.single().is_ok_and(|current| *current == expected)
}

/// Local-only marker placed on projectile simulation owned by the server.
/// It is intentionally not registered for replication: clients should render
/// replicated state, not accidentally run server hit detection on it.
#[derive(Component)]
pub(crate) struct AuthoritativeProjectile;

/// Exact world-space result of a successful collision query.
///
/// This is deliberately local-only presentation: server-current and rewound
/// policies create it on the server, while client-reported collision creates
/// it on the shooting client.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct HitImpact {
    pub(crate) position: Vec2,
    pub(crate) normal: Vec2,
}

const IMPACT_LIFETIME_SECONDS: f32 = 0.65;

/// Retain a collision result long enough for a GUI app to draw a visible cross
/// at the exact ray intersection point.
pub(crate) fn remember_impact(
    commands: &mut Commands,
    origin: Vec2,
    direction: Dir2,
    hit: RayHitData,
) {
    let position = origin + direction.as_vec2() * hit.distance;
    commands.spawn((
        HitImpact {
            position,
            normal: hit.normal,
        },
        DespawnAfter(Timer::from_seconds(
            IMPACT_LIFETIME_SECONDS,
            TimerMode::Once,
        )),
        Name::new("Projectile impact point"),
    ));
}
