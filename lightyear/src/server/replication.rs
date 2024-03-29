use crate::client::components::Confirmed;
use crate::client::interpolation::Interpolated;
use crate::client::prediction::Predicted;
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::prelude::{MainSet, Protocol, ReplicationSet, SharedConfig, Tick};
use crate::server::connection::ConnectionManager;
use crate::server::prediction::compute_hash;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::plugin::ReplicationPlugin;

/// Configuration related to replicating the server's World to clients
#[derive(Clone, Default, Debug)]
pub struct ReplicationConfig {
    /// Set to true to disable replicating this server's entities to clients
    pub disable: bool,
}

pub struct ServerReplicationPlugin<P: Protocol> {
    tick_duration: Duration,
    marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> ServerReplicationPlugin<P> {
    pub(crate) fn new(tick_duration: Duration) -> Self {
        Self {
            tick_duration,
            marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol> Plugin for ServerReplicationPlugin<P> {
    fn build(&self, app: &mut App) {
        app
            // PLUGIN
            .add_plugins(ReplicationPlugin::<P, ConnectionManager<P>>::new(
                self.tick_duration,
            ))
            // SYSTEM SETS
            .configure_sets(
                PreUpdate,
                (MainSet::ClientReplication, MainSet::ClientReplicationFlush)
                    .chain()
                    .after(MainSet::ReceiveFlush),
            )
            .configure_sets(
                PostUpdate,
                ((
                    // on server: we need to set the hash value before replicating the component
                    ReplicationSet::SetPreSpawnedHash.before(ReplicationSet::SendComponentUpdates),
                )
                    .in_set(ReplicationSet::All),),
            )
            // SYSTEMS
            .add_systems(
                PostUpdate,
                (compute_hash::<P>.in_set(ReplicationSet::SetPreSpawnedHash),),
            );
    }
}
