use crate::Deterministic;
use bevy_app::{App, Plugin};
use lightyear_prediction::rollback::DeterministicPredicted;

/// Shared setup for deterministic replication — automatically inserted by
/// [`ChecksumSendPlugin`], [`ChecksumReceivePlugin`] and
/// [`LateJoinCatchUpPlugin`] so users rarely need to add it directly.
///
/// [`ChecksumSendPlugin`]: crate::prelude::ChecksumSendPlugin
/// [`ChecksumReceivePlugin`]: crate::prelude::ChecksumReceivePlugin
/// [`LateJoinCatchUpPlugin`]: crate::prelude::LateJoinCatchUpPlugin
pub struct DeterministicReplicationPlugin;

impl Plugin for DeterministicReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<DeterministicPredicted, Deterministic>();
    }
}
