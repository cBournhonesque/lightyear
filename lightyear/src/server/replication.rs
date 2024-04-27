use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;

use crate::_reexport::ServerMarker;
use crate::client::components::Confirmed;
use crate::client::interpolation::Interpolated;
use crate::client::prediction::Predicted;
use crate::connection::client::NetClient;
use crate::prelude::client::ClientConnection;
use crate::prelude::{Mode, PrePredicted, Protocol, SharedConfig};
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::networking::is_started;
use crate::server::prediction::compute_hash;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::plugin::ReplicationPlugin;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

/// Configuration related to replicating the server's World to clients
#[derive(Clone, Debug)]
pub struct ReplicationConfig {
    /// Set to true to disable replicating this server's entities to clients
    pub enable_send: bool,
    pub enable_receive: bool,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            enable_send: true,
            enable_receive: false,
        }
    }
}

#[derive(Default)]
pub struct ServerReplicationPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ServerReplicationSet {
    /// You can use this SystemSet to add Replicate components to entities received from clients (to rebroadcast them to other clients)
    ClientReplication,
}

impl Plugin for ServerReplicationPlugin {
    fn build(&self, app: &mut App) {
        let config = app.world.resource::<ServerConfig>();

        app
            // PLUGIN
            // .add_plugins(ReplicationPlugin::<P, ConnectionManager>::new(
            //     config.shared.tick.tick_duration,
            //     config.replication.enable_send,
            //     config.replication.enable_receive,
            // ))
            // SYSTEM SETS
            .configure_sets(
                PreUpdate,
                ServerReplicationSet::ClientReplication
                    .run_if(is_started)
                    .after(InternalMainSet::<ServerMarker>::Receive),
            )
            .configure_sets(
                PostUpdate,
                // on server: we need to set the hash value before replicating the component
                InternalReplicationSet::<ServerMarker>::SetPreSpawnedHash
                    .before(InternalReplicationSet::<ServerMarker>::SendComponentUpdates)
                    .in_set(InternalReplicationSet::<ServerMarker>::All),
            )
            // SYSTEMS
            .add_systems(
                PostUpdate,
                compute_hash.in_set(InternalReplicationSet::<ServerMarker>::SetPreSpawnedHash),
            );

        // HOST-SERVER
        app.add_systems(
            PostUpdate,
            add_prediction_interpolation_components
                .after(InternalMainSet::<ServerMarker>::Send)
                .run_if(SharedConfig::is_host_server_condition),
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

/// In HostServer mode, we will add the Predicted/Interpolated components to the server entities
/// So that client code can still query for them
fn add_prediction_interpolation_components(
    mut commands: Commands,
    query: Query<(Entity, Ref<Replicate>, Option<&PrePredicted>)>,
    connection: Res<ClientConnection>,
) {
    let local_client = connection.id();
    for (entity, replicate, pre_predicted) in query.iter() {
        if (replicate.is_added() || replicate.is_changed())
            && replicate.replication_target.should_send_to(&local_client)
        {
            if pre_predicted.is_some_and(|pre_predicted| pre_predicted.client_entity.is_none()) {
                // PrePredicted's client_entity is None if it's a pre-predicted entity that was spawned by the local client
                // in that case, just remove it and add Predicted instead
                commands
                    .entity(entity)
                    .insert(Predicted {
                        confirmed_entity: Some(entity),
                    })
                    .remove::<PrePredicted>();
            }
            if replicate.prediction_target.should_send_to(&local_client) {
                commands.entity(entity).insert(Predicted {
                    confirmed_entity: Some(entity),
                });
            }
            if replicate.interpolation_target.should_send_to(&local_client) {
                commands.entity(entity).insert(Interpolated {
                    confirmed_entity: entity,
                });
            }
        }
    }
}
