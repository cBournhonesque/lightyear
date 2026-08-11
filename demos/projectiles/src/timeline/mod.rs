//! Player and projectile presentation-timeline axis.
//!
//! This axis describes which entities clients predict or interpolate and
//! which time should be used to render projectile presentation. It must not
//! silently select the hit authority.
//!
//! The server-owned `TimelinePolicy` component selects one of these independent
//! presentation policies.

use bevy::prelude::{Component, EntityCommands, Reflect};
use lightyear::prelude::PeerId;
use serde::{Deserialize, Serialize};

pub(crate) mod all_interpolated;
pub(crate) mod all_predicted;
pub(crate) mod owner_predicted;

/// Selects how each client presents player timelines.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect, Serialize, Deserialize,
)]
pub(crate) enum TimelinePolicy {
    /// Predict the local owner and interpolate remote players.
    #[default]
    OwnerPredictedRemotesInterpolated,
    /// Predict local and remote players from replicated inputs.
    AllPredicted,
    /// Interpolate every player, including the local owner.
    AllInterpolated,
}

impl TimelinePolicy {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::OwnerPredictedRemotesInterpolated => Self::AllPredicted,
            Self::AllPredicted => Self::AllInterpolated,
            Self::AllInterpolated => Self::OwnerPredictedRemotesInterpolated,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::OwnerPredictedRemotesInterpolated => "Owner predicted, remotes interpolated",
            Self::AllPredicted => "All players predicted",
            Self::AllInterpolated => "All players interpolated",
        }
    }

    /// Configure the prediction/interpolation targets for one server-owned
    /// player. The concrete policies stay in their named files so their
    /// trade-offs and behavior can be read together.
    pub(crate) fn configure_player(self, entity: &mut EntityCommands, owner: PeerId) {
        match self {
            Self::OwnerPredictedRemotesInterpolated => {
                owner_predicted::configure_player(entity, owner)
            }
            Self::AllPredicted => all_predicted::configure_player(entity),
            Self::AllInterpolated => all_interpolated::configure_player(entity),
        }
    }

    /// Configure a server projectile for the same presentation policy.
    pub(crate) fn configure_projectile(self, entity: &mut EntityCommands, owner: PeerId) {
        match self {
            Self::OwnerPredictedRemotesInterpolated => {
                owner_predicted::configure_projectile(entity, owner)
            }
            Self::AllPredicted => all_predicted::configure_projectile(entity, owner),
            Self::AllInterpolated => all_interpolated::configure_projectile(entity),
        }
    }

    /// The all-interpolated policy waits for the authoritative server shot.
    /// The other two policies give the owner an immediate local visual.
    pub(crate) fn owner_spawns_locally(self) -> bool {
        !matches!(self, Self::AllInterpolated)
    }
}
