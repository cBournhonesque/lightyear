use crate::_reexport::ServerMarker;
use crate::client::components::Confirmed;
use crate::client::interpolation::Interpolated;
use crate::client::prediction::Predicted;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::prelude::{Protocol, SharedConfig, Tick};
use crate::server::connection::ConnectionManager;
use crate::server::prediction::compute_hash;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::plugin::ReplicationPlugin;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

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

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ServerReplicationSet {
    /// You can use this SystemSet to add Replicate components to entities received from clients (to rebroadcast them to other clients)
    ClientReplication,
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
                ServerReplicationSet::ClientReplication
                    .after(InternalMainSet::<ServerMarker>::Receive),
            )
            .configure_sets(
                PostUpdate,
                ((
                    // on server: we need to set the hash value before replicating the component
                    InternalReplicationSet::<ServerMarker>::SetPreSpawnedHash
                        .before(InternalReplicationSet::<ServerMarker>::SendComponentUpdates),
                )
                    .in_set(InternalReplicationSet::<ServerMarker>::All),),
            )
            // SYSTEMS
            .add_systems(
                PostUpdate,
                (compute_hash::<P>
                    .in_set(InternalReplicationSet::<ServerMarker>::SetPreSpawnedHash),),
            );
    }
}

/// Filter to use to get all entities that are not client-side replicated entities
#[derive(QueryFilter)]
pub struct ServerFilter {
    a: (
        Without<Confirmed>,
        Without<Predicted>,
        Without<Interpolated>,
    ),
}
