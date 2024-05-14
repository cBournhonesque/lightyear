//! Client replication plugins
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::connection::ConnectionManager;
use crate::client::networking::is_connected;
use crate::client::sync::client_is_synced;
use crate::prelude::SharedConfig;
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

pub(crate) mod receive {
    use super::*;
    #[derive(Default)]
    pub struct ClientReplicationReceivePlugin {
        pub tick_interval: Duration,
    }

    impl Plugin for ClientReplicationReceivePlugin {
        fn build(&self, app: &mut App) {
            // PLUGIN
            app.add_plugins(ReplicationReceivePlugin::<ConnectionManager>::new(
                self.tick_interval,
            ));

            // TODO: currently we only support pre-spawned entities spawned during the FixedUpdate schedule
            // // SYSTEM SETS
            // .configure_sets(
            //     PostUpdate,
            //     // on client, the client hash component is not replicated to the server, so there's no ordering constraint
            //     ReplicationSet::SetPreSpawnedHash.in_set(ReplicationSet::All),
            // )

            app.configure_sets(
                PostUpdate,
                // only replicate entities once client is synced
                // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
                //  and the message might be ignored by the server
                //  But then pre-predicted entities that are spawned right away will not be replicated?
                // NOTE: we always need to add this condition if we don't enable replication, because
                InternalReplicationSet::<ClientMarker>::All.run_if(
                    is_connected
                        .and_then(client_is_synced)
                        .and_then(not(SharedConfig::is_host_server_condition)),
                ),
            );
        }
    }
}

pub(crate) mod send {
    use super::*;
    use crate::prelude::server::ControlledBy;
    use crate::prelude::{
        ClientId, ComponentRegistry, ReplicateHierarchy, ReplicationGroup, ShouldBePredicted,
        TargetEntity, VisibilityMode,
    };
    use crate::server::replication::send::SyncTarget;
    use crate::server::visibility::immediate::{ClientVisibility, ReplicateVisibility};
    use crate::shared::replication::components::{
        Controlled, ReplicationTarget, ShouldBeInterpolated,
    };
    use crate::shared::replication::network_target::NetworkTarget;
    use crate::shared::replication::ReplicationSend;

    #[derive(Default)]
    pub struct ClientReplicationSendPlugin {
        pub tick_interval: Duration,
    }

    impl Plugin for ClientReplicationSendPlugin {
        fn build(&self, app: &mut App) {
            app
                // PLUGIN
                .add_plugins(ReplicationSendPlugin::<ConnectionManager>::new(
                    self.tick_interval,
                ))
                // SETS
                .configure_sets(
                    PostUpdate,
                    // only replicate entities once client is synced
                    // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
                    //  and the message might be ignored by the server
                    //  But then pre-predicted entities that are spawned right away will not be replicated?
                    // NOTE: we always need to add this condition if we don't enable replication, because
                    InternalReplicationSet::<ClientMarker>::All.run_if(
                        is_connected
                            .and_then(client_is_synced)
                            .and_then(not(SharedConfig::is_host_server_condition)),
                    ),
                )
                // SYSTEMS
                .add_systems(
                    PostUpdate,
                    send_entity_spawn
                        .in_set(InternalReplicationSet::<ClientMarker>::BufferEntityUpdates),
                );
        }
    }

    /// Bundle that indicates how an entity should be replicated. Add this to an entity to start replicating
    /// it to the server.
    ///
    /// ```rust
    /// use bevy::prelude::*;
    /// use lightyear::prelude::*;
    /// use lightyear::prelude::client::*;
    ///
    /// let mut world = World::default();
    /// world.spawn(ReplicateToServer::default());
    /// ```
    ///
    /// The bundle is composed of several components:
    /// - [`ReplicationTarget`] to specify if the entity should be replicated to the server or not
    /// - [`ReplicationGroup`] to group entities together for replication. Entities in the same group
    /// will be sent together in the same message.
    /// - [`ReplicateHierarchy`] to specify how the hierarchy of the entity should be replicated
    #[derive(Bundle, Clone, Default, PartialEq, Debug, Reflect)]
    pub struct ReplicateToServer {
        /// Should the entity be replicated to the client?
        pub target: ReplicationTarget,
        /// The replication group defines how entities are grouped (sent as a single message) for replication.
        ///
        /// After the entity is first replicated, the replication group of the entity should not be modified.
        /// (but more entities can be added to the replication group)
        // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
        //  it just keeps living but doesn't receive any updates. Should we make this configurable?
        pub group: ReplicationGroup,
        /// How should the hierarchy of the entity (parents/children) be replicated?
        pub hierarchy: ReplicateHierarchy,
    }

    /// Send entity spawn replication messages to server when the ReplicationTarget component is added
    /// Also handles:
    /// - handles TargetEntity if it's a Preexisting entity
    /// - setting the priority
    pub(crate) fn send_entity_spawn(
        query: Query<
            (
                Entity,
                Ref<ReplicationTarget>,
                &ReplicationGroup,
                Option<&TargetEntity>,
            ),
            Changed<ReplicationTarget>,
        >,
        mut sender: ResMut<ConnectionManager>,
    ) {
        query
            .iter()
            .for_each(|(entity, replication_target, group, target_entity)| {
                let mut target = replication_target.target.clone();
                if !replication_target.is_added() {
                    if let Some(cached_replicate) = sender.replicate_component_cache.get(&entity) {
                        // do not re-send a spawn message to the server if we already have sent one
                        target.exclude(&cached_replicate.replication_target)
                    }
                }
                if target.is_empty() {
                    return;
                }
                trace!(?entity, "Prepare entity spawn to server");
                let group_id = group.group_id(Some(entity));
                if let Some(TargetEntity::Preexisting(remote_entity)) = target_entity {
                    sender.replication_sender.prepare_entity_spawn_reuse(
                        entity,
                        group_id,
                        *remote_entity,
                    );
                } else {
                    sender
                        .replication_sender
                        .prepare_entity_spawn(entity, group_id);
                }
                // TODO: should the priority be a component on the entity? but it should be shared between a group
                //  should a GroupChannel be a separate entity?
                // also set the priority for the group when we spawn it
                sender
                    .replication_sender
                    .update_base_priority(group_id, group.priority());
            });
    }
}
