//! All-players-predicted timeline policy.
//!
//! Every client predicts every player from replicated inputs, including
//! players it does not control.
//!
//! # Advantages
//!
//! - Remote movement can appear more current than interpolation.
//! - Current-state hit detection may visually disagree less when prediction is
//! accurate.
//! - Demonstrates Lightyear input rebroadcast and remote prediction.
//!
//! # Trade-offs
//!
//! - Missing or late remote inputs require guesses and later correction.
//! - Prediction and rollback cost grows with every predicted player.
//! - Predicting movement does not automatically define how remote firing,
//!   collision, or outcomes reconcile.

use bevy::prelude::*;
use lightyear::prelude::*;

pub(super) fn configure_player(entity: &mut EntityCommands) {
    #[cfg(feature = "server")]
    entity.insert(PredictionTarget::to_clients(NetworkTarget::All));
}

pub(super) fn configure_projectile(entity: &mut EntityCommands, owner: PeerId) {
    // The owner has an immediate PreSpawned copy. Its sender-scoped signature
    // matches only there; every other client creates a normal predicted entity.
    #[cfg(feature = "server")]
    entity.insert((
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::All),
    ));
}
