use crate::Deterministic;
use bevy_app::{App, Plugin};
use lightyear_prediction::rollback::DeterministicPredicted;

/// Shared setup for deterministic replication.
///
/// This is automatically inserted by [`ChecksumPlugin`],
/// [`ChecksumSendPlugin`] and [`ChecksumReceivePlugin`], so users normally add
/// [`ChecksumPlugin`] from their shared protocol when they want checksum
/// verification.
///
/// [`ChecksumPlugin`]: crate::prelude::ChecksumPlugin
/// [`ChecksumSendPlugin`]: crate::prelude::ChecksumSendPlugin
/// [`ChecksumReceivePlugin`]: crate::prelude::ChecksumReceivePlugin
pub struct DeterministicReplicationPlugin;

impl Plugin for DeterministicReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<DeterministicPredicted, Deterministic>();
    }
}
