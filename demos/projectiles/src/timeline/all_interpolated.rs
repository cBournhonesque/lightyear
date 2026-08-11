//! All-players-interpolated timeline policy.
//!
//! Every player, including the locally controlled one, is displayed from an
//! interpolation timeline rather than local prediction.
//!
//! # Advantages
//!
//! - All visible actors can share a stable delayed presentation timeline.
//! - Avoids local prediction rollback and correction artifacts.
//! - Useful as a comparison for presentation timing.
//!
//! # Trade-offs
//!
//! - Local movement and firing feel delayed unless presentation is decoupled
//!   from the interpolated body.
//! - Equal render timelines do not by themselves eliminate input-to-server
//!   latency or the need to define hit timing.
//! - Local input ownership and projectile origin semantics require special
//!   handling.

use bevy::prelude::*;
use lightyear::prelude::*;

pub(super) fn configure_player(entity: &mut EntityCommands) {
    #[cfg(feature = "server")]
    entity.insert(InterpolationTarget::to_clients(NetworkTarget::All));
}

pub(super) fn configure_projectile(entity: &mut EntityCommands) {
    // Nobody creates an immediate local copy in this mode, including the
    // owner. Everyone renders the authoritative entity after interpolation.
    #[cfg(feature = "server")]
    entity.insert((
        Replicate::to_clients(NetworkTarget::All),
        InterpolationTarget::to_clients(NetworkTarget::All),
    ));
}
