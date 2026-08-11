//! Owner-predicted, remotes-interpolated timeline policy.
//!
//! Each client predicts its locally controlled player and renders remote
//! players from the interpolation timeline. Remote projectiles should also be
//! reconstructed on that remote timeline rather than fast-forwarded to the
//! owner's predicted time.
//!
//! # Advantages
//!
//! - Responsive local movement and firing with smooth remote motion.
//! - Matches the conventional authoritative client/server topology.
//! - Prediction scope and reconciliation cost stay focused on the owner.
//!
//! # Trade-offs
//!
//! - The shooter sees remote targets in the past, so server-current hit
//!   detection creates target advantage.
//! - Local and remote projectiles use different presentation times.
//! - Owner prediction must reconcile authoritative fire data and outcomes.

use bevy::prelude::*;
use lightyear::prelude::*;

pub(super) fn configure_player(entity: &mut EntityCommands, owner: PeerId) {
    #[cfg(feature = "server")]
    entity.insert((
        PredictionTarget::to_clients(NetworkTarget::Single(owner)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(owner)),
    ));
}

pub(super) fn configure_projectile(entity: &mut EntityCommands, owner: PeerId) {
    // The owner already spawned an immediate PreSpawned projectile. Replicate
    // to everyone so Lightyear can match that local entity for the owner while
    // creating ordinary interpolated entities for remote clients.
    #[cfg(feature = "server")]
    entity.insert((
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(owner)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(owner)),
    ));
}
