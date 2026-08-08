//! Projectile network-representation axis.
//!
//! This axis describes which projectile facts are replicated. It must not
//! change the canonical trajectory or authoritative hit result.
//!
//! The active implementations are `StateEntity`, `FireDataEntity`, and
//! `ShotBuffer`.

use bevy::prelude::{Component, Reflect};
use lightyear::prelude::{PeerId, Tick};
use serde::{Deserialize, Serialize};

pub(crate) mod fire_data_entity;
pub(crate) mod shot_buffer;
pub(crate) mod state_entity;

/// Selects how authoritative projectile data is represented on the network.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect, Serialize, Deserialize,
)]
pub(crate) enum RepresentationKind {
    /// Replicate a network entity and its changing state.
    #[default]
    StateEntity,
    /// Replicate one entity containing immutable or sparse fire data.
    FireDataEntity,
    /// Replicate a bounded stream of shots on their shooter or weapon.
    ShotBuffer,
}

impl RepresentationKind {
    /// Cycle through every implemented network representation.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::StateEntity => Self::FireDataEntity,
            Self::FireDataEntity => Self::ShotBuffer,
            Self::ShotBuffer => Self::StateEntity,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::StateEntity => "State entity",
            Self::FireDataEntity => "Fire-data entity",
            Self::ShotBuffer => "Shot buffer",
        }
    }
}

/// Build the explicit signature used to match an owner's local projectile to
/// the server's authoritative entity.
///
/// There is at most one accepted shot per shooter per tick in this example,
/// so shooter + tick + the selected trajectory/representation is unique. If a
/// future shotgun creates several entities in one tick, its pellet ordinal
/// must be mixed in here too. This is a matching signature, not a `ShotId`
/// component or a separate identity entity.
pub(crate) fn prespawn_hash(
    shooter: PeerId,
    fire_tick: Tick,
    trajectory: crate::trajectory::TrajectoryKind,
    representation: RepresentationKind,
) -> u64 {
    let trajectory_tag = match trajectory {
        crate::trajectory::TrajectoryKind::Hitscan => 0x4849_5453_4341_4e00,
        crate::trajectory::TrajectoryKind::Linear => 0x4c49_4e45_4152_0000,
    };
    let representation_tag = match representation {
        RepresentationKind::StateEntity => 0x5354_4154_4500_0000,
        RepresentationKind::FireDataEntity => 0x4649_5245_4441_5441,
        RepresentationKind::ShotBuffer => 0x4255_4646_4552_0000,
    };

    // The same small integer mixer is deterministic in every process and does
    // not depend on Rust's randomized HashMap state.
    let mut hash = shooter.to_bits() ^ (fire_tick.0 as u64).rotate_left(21);
    hash ^= trajectory_tag;
    hash = hash.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    hash ^ representation_tag
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trajectory::TrajectoryKind;

    #[test]
    fn prespawn_signature_is_stable_and_separates_shots() {
        let shooter = PeerId::Local(7);
        let tick = Tick(42);
        let base = prespawn_hash(
            shooter,
            tick,
            TrajectoryKind::Linear,
            RepresentationKind::StateEntity,
        );

        assert_eq!(
            base,
            prespawn_hash(
                shooter,
                tick,
                TrajectoryKind::Linear,
                RepresentationKind::StateEntity,
            )
        );
        assert_ne!(
            base,
            prespawn_hash(
                PeerId::Local(8),
                tick,
                TrajectoryKind::Linear,
                RepresentationKind::StateEntity,
            )
        );
        assert_ne!(
            base,
            prespawn_hash(
                shooter,
                Tick(43),
                TrajectoryKind::Linear,
                RepresentationKind::StateEntity,
            )
        );
        assert_ne!(
            base,
            prespawn_hash(
                shooter,
                tick,
                TrajectoryKind::Hitscan,
                RepresentationKind::StateEntity,
            )
        );
        assert_ne!(
            base,
            prespawn_hash(
                shooter,
                tick,
                TrajectoryKind::Linear,
                RepresentationKind::FireDataEntity,
            )
        );
    }
}
