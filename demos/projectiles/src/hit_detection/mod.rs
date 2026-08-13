//! Projectile hit-policy axis.
//!
//! This axis describes which peer decides a hit and which point in world
//! history it queries. The selected policy consumes the same projectile
//! markers regardless of which network representation created them.
//!
//! The server-owned `HitPolicy` component selects exactly one implementation.

use avian2d::prelude::RayHitData;
use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::protocol::{Bot, ClientContext, PlayerMarker, Score};
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
/// It is also replicated from the server after a hit is accepted so an
/// impact produced by a headless bot or server can be drawn in GUI clients.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct HitImpact {
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
) -> HitImpact {
    let impact = impact_from_hit(origin, direction, hit);
    remember_impact_value(commands, impact);
    impact
}

pub(crate) fn impact_from_hit(origin: Vec2, direction: Dir2, hit: RayHitData) -> HitImpact {
    HitImpact {
        position: origin + direction.as_vec2() * hit.distance,
        normal: hit.normal,
    }
}

/// Retain an already-computed impact received from another app.
pub(crate) fn remember_impact_value(commands: &mut Commands, impact: HitImpact) {
    commands.spawn((
        impact,
        DespawnAfter(Timer::from_seconds(
            IMPACT_LIFETIME_SECONDS,
            TimerMode::Once,
        )),
        Name::new("Projectile impact point"),
    ));
}

/// Replicate an accepted impact so GUI clients can draw collisions performed
/// by the authoritative server or the embedded bot's headless client.
fn publish_impact(commands: &mut Commands, impact: HitImpact) {
    commands.spawn((
        impact,
        Replicate::to_clients(NetworkTarget::All),
        DespawnAfter(Timer::from_seconds(
            IMPACT_LIFETIME_SECONDS,
            TimerMode::Once,
        )),
        Name::new("Authoritative projectile impact point"),
    ));
}

/// Apply the authoritative gameplay result and publish its debug geometry.
pub(crate) fn accept_hit(
    commands: &mut Commands,
    shooter: Entity,
    target: Entity,
    impact: HitImpact,
    bots: &Query<(), With<Bot>>,
    scores: &mut Query<&mut Score, With<PlayerMarker>>,
) {
    publish_impact(commands, impact);
    let scored_player = if bots.contains(shooter) {
        if let Ok(mut score) = scores.get_mut(target) {
            score.0 -= 1;
            Some((target, score.0, -1))
        } else {
            None
        }
    } else if let Ok(mut score) = scores.get_mut(shooter) {
        score.0 += 1;
        Some((shooter, score.0, 1))
    } else {
        None
    };
    if let Some((player, score, delta)) = scored_player {
        debug!(?player, score, delta, "Applied projectile score change");
    }
}
