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
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, ServerMarker};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ServerReplicationSet {
    // You can use this SystemSet to add Replicate components to entities received from clients (to rebroadcast them to other clients)
    ClientReplication,
}

mod receive {
    use super::*;

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
}

mod send {
    use super::*;
    use crate::prelude::{
        ComponentRegistry, ReplicationGroup, ShouldBePredicted, TargetEntity, VisibilityMode,
    };
    use crate::server::visibility::immediate::{ClientVisibility, ReplicateVisibility};
    use crate::shared::replication::components::{
        Controlled, ControlledBy, ReplicationTarget, ShouldBeInterpolated,
    };
    use crate::shared::replication::network_target::NetworkTarget;
    use crate::shared::replication::ReplicationSend;
    use bevy::ecs::system::SystemChangeTick;

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
                // TODO: putting it here means we might miss entities that are spawned and despawned within the send_interval? bug or feature?
                //  be careful that newly_connected_client is cleared every send_interval, not every frame.
                send_entity_spawn
                    .in_set(InternalReplicationSet::<ServerMarker>::BufferEntityUpdates),
            );
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
        query: Query<(Entity, Ref<ReplicationTarget>, Option<&PrePredicted>)>,
        connection: Res<ClientConnection>,
    ) {
        let local_client = connection.id();
        for (entity, replication_target, pre_predicted) in query.iter() {
            if (replication_target.is_changed()) && replication_target.targets(&local_client) {
                if pre_predicted.is_some_and(|pre_predicted| pre_predicted.client_entity.is_none())
                {
                    // PrePredicted's client_entity is None if it's a pre-predicted entity that was spawned by the local client
                    // in that case, just remove it and add Predicted instead
                    commands
                        .entity(entity)
                        .insert(Predicted {
                            confirmed_entity: Some(entity),
                        })
                        .remove::<PrePredicted>();
                }
                if replication_target.prediction.targets(&local_client) {
                    commands.entity(entity).insert(Predicted {
                        confirmed_entity: Some(entity),
                    });
                }
                if replication_target.interpolation.targets(&local_client) {
                    commands.entity(entity).insert(Interpolated {
                        confirmed_entity: entity,
                    });
                }
            }
        }
    }

    /// Send entity spawn replication messages to clients
    /// Also handles:
    /// - newly_connected_clients should receive the entity spawn message even if the entity was not just spawned
    /// - adds ControlledBy, ShouldBePredicted, ShouldBeInterpolated component
    /// - handles TargetEntity if it's a Preexisting entity
    pub(crate) fn send_entity_spawn(
        component_registry: Res<ComponentRegistry>,
        query: Query<(
            Entity,
            Ref<ReplicationTarget>,
            &ReplicationGroup,
            &ControlledBy,
            Option<&TargetEntity>,
            Option<&ReplicateVisibility>,
        )>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        // Replicate to already connected clients (replicate only new entities)
        query.iter().for_each(|(entity, replication_target, group, controlled_by, target_entity, visibility )| {
            let target = match visibility {
                // for room mode, no need to handle newly-connected clients specially; they just need
                // to be added to the correct room
                Some(visibility) => {
                    visibility.clients_cache
                        .iter()
                        .filter_map(|(client_id, visibility)| {
                            if replication_target.replication.targets(client_id) {
                                match visibility {
                                    ClientVisibility::Gained => {
                                        trace!(
                                        ?entity,
                                        ?client_id,
                                        "send entity spawn to client who just gained visibility"
                                        );
                                        return Some(*client_id);
                                    }
                                    ClientVisibility::Lost => {}
                                    ClientVisibility::Maintained => {
                                        // only try to replicate if the replicate component was just added
                                        if replication_target.is_added() {
                                            trace!(
                                                ?entity,
                                                ?client_id,
                                                "send entity spawn to client who maintained visibility"
                                            );
                                            return Some(*client_id);
                                        }
                                    }
                                }
                            }
                            return None;
                        }).collect()
                }
                None => {
                    let mut target = NetworkTarget::None;
                    // only try to replicate if the replicate component was just added
                    if replication_target.is_added() {
                        trace!(?entity, "send entity spawn");
                        target.union(&replication_target.replication);
                    }

                    // also replicate to the newly connected clients that match the target
                    let new_connected_clients = sender.new_connected_clients();
                    if !new_connected_clients.is_empty() {
                        // replicate to the newly connected clients that match our target
                        let mut new_connected_target = NetworkTarget::Only(new_connected_clients);
                        new_connected_target.intersection(&replication_target.replication);
                        debug!(?entity, target = ?new_connected_target, "Replicate to newly connected clients");
                        target.union(&new_connected_target);
                    }
                    target
                }
            };
            if target.is_empty() {
                return;
            }
            trace!(?entity, "Prepare entity spawn to client");
            let group_id = group.group_id(Some(entity));
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            let _ = sender.apply_replication(target).try_for_each(|client_id| {
                // let the client know that this entity is controlled by them
                if controlled_by.targets(&client_id) {
                    sender.prepare_typed_component_insert(entity, group_id, client_id, component_registry.as_ref(), &Controlled)?;
                }
                // if we need to do prediction/interpolation, send a marker component to indicate that to the client
                if replication_target.prediction.targets(&client_id) {
                    // TODO: the serialized data is always the same; cache it somehow?
                    sender.prepare_typed_component_insert(
                        entity,
                        group_id,
                        client_id,
                        component_registry.as_ref(),
                        &ShouldBePredicted,
                    )?;
                }
                if replication_target.interpolation.targets(&client_id) {
                    sender.prepare_typed_component_insert(
                        entity,
                        group_id,
                        client_id,
                        component_registry.as_ref(),
                        &ShouldBeInterpolated,
                    )?;
                }

                if let Some(TargetEntity::Preexisting(remote_entity)) = target_entity {
                    sender.connection_mut(client_id)?.replication_sender.prepare_entity_spawn_reuse(
                        entity,
                        group_id,
                        *remote_entity,
                    );
                } else {
                    sender.connection_mut(client_id)?.replication_sender
                        .prepare_entity_spawn(entity, group_id);
                }

                // also set the priority for the group when we spawn it
                sender.connection_mut(client_id)?.replication_sender.update_base_priority(group_id, group.priority())?;
                Ok(())
            }).inspect_err(|e| {
                error!("error sending entity spawn: {:?}", e);
            });
        });
    }
}
