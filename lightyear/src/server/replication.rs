use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::components::Confirmed;
use crate::client::interpolation::Interpolated;
use crate::client::prediction::Predicted;
use crate::connection::client::NetClient;
use crate::prelude::client::ClientConnection;
use crate::prelude::{PrePredicted, SharedConfig};
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::networking::is_started;
use crate::server::prediction::compute_hash;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, ServerMarker};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ServerReplicationSet {
    // You can use this SystemSet to add Replicate components to entities received from clients (to rebroadcast them to other clients)
    ClientReplication,
}

#[derive(Default)]
pub struct ServerReplicationReceivePlugin {
    pub tick_interval: Duration,
}

impl Plugin for ServerReplicationReceivePlugin {
    fn build(&self, app: &mut App) {
        app
            // PLUGIN
            .add_plugins(ReplicationReceivePlugin::<ConnectionManager>::new(
                self.tick_interval,
            ))
            // SETS
            .configure_sets(
                PreUpdate,
                ServerReplicationSet::ClientReplication
                    .run_if(is_started)
                    .after(InternalMainSet::<ServerMarker>::EmitEvents),
            );
    }
}

#[derive(Default)]
pub struct ServerReplicationSendPlugin {
    pub tick_interval: Duration,
}

impl Plugin for ServerReplicationSendPlugin {
    fn build(&self, app: &mut App) {
        let config = app.world.resource::<ServerConfig>();

        app
            // PLUGIN
            .add_plugins(ReplicationSendPlugin::<ConnectionManager>::new(
                self.tick_interval,
            ))
            // SYSTEM SETS
            .configure_sets(
                PostUpdate,
                // on server: we need to set the hash value before replicating the component
                InternalReplicationSet::<ServerMarker>::SetPreSpawnedHash
                    .before(InternalReplicationSet::<ServerMarker>::BufferComponentUpdates)
                    .in_set(InternalReplicationSet::<ServerMarker>::All),
            )
            .configure_sets(
                PostUpdate,
                InternalReplicationSet::<ServerMarker>::All.run_if(is_started),
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
            && replicate.replication_target.targets(&local_client)
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
            if replicate.prediction_target.targets(&local_client) {
                commands.entity(entity).insert(Predicted {
                    confirmed_entity: Some(entity),
                });
            }
            if replicate.interpolation_target.targets(&local_client) {
                commands.entity(entity).insert(Interpolated {
                    confirmed_entity: entity,
                });
            }
        }
    }
}
