//! Projectile trajectory axis.
//!
//! This axis describes how a shot moves through the simulation. It should not
//! decide how the shot is replicated, which timeline is rendered, or who is
//! authoritative for hit detection.
//!
//! Each concrete trajectory lives in its own module. Shared firing code only
//! selects one of them; it does not contain trajectory-specific simulation.

use bevy::prelude::{Component, Reflect};
use serde::{Deserialize, Serialize};

pub(crate) mod hitscan;
pub(crate) mod linear;

/// Selects the mathematical trajectory used by a shot.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect, Serialize, Deserialize,
)]
pub(crate) enum TrajectoryKind {
    /// An instantaneous ray query.
    #[default]
    Hitscan,
    /// A constant-velocity projectile advanced over fixed ticks.
    Linear,
}

impl TrajectoryKind {
    /// Cycle only through trajectories that this example currently implements.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Hitscan => Self::Linear,
            Self::Linear => Self::Hitscan,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Hitscan => "Hitscan",
            Self::Linear => "Linear projectile",
        }
    }

    /// Shots per second. Keeping this here makes cadence a trajectory concern
    /// instead of scattering weapon-specific constants through networking code.
    pub(crate) fn fire_rate(self) -> f32 {
        match self {
            Self::Hitscan => 5.0,
            Self::Linear => 2.0,
        }
    }
}
