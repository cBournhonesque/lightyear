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

pub(crate) mod receive {
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

pub(crate) mod send {
    use super::*;
    use crate::prelude::{
        ClientId, ComponentRegistry, DisabledComponent, OverrideTargetComponent,
        ReplicateHierarchy, ReplicateOnceComponent, ReplicationGroup, ShouldBePredicted,
        TargetEntity, VisibilityMode,
    };
    use crate::server::visibility::immediate::{ClientVisibility, ReplicateVisibility};
    use crate::shared::replication::components::{
        Controlled, DespawnTracker, Replicating, ReplicationTarget, ShouldBeInterpolated,
    };
    use crate::shared::replication::network_target::NetworkTarget;
    use crate::shared::replication::{systems, ReplicationSend};
    use bevy::ecs::entity::Entities;
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
            // SYSTEMS
            app.add_systems(
                PreUpdate,
                // we need to add despawn trackers immediately for entities for which we add replicate
                // TODO: why?
                handle_replicating_add.after(ServerReplicationSet::ClientReplication),
            );
            app.add_systems(
                PostUpdate,
                (
                    // NOTE: we need to run `send_entity_despawn` once per frame (and not once per send_interval)
                    //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
                    //  It is ok to run it every frame because it creates at most one message per despawn
                    // NOTE: we make sure to update the replicate_cache before we make use of it in `send_entity_despawn`
                    handle_replicating_remove
                        .in_set(InternalReplicationSet::<ServerMarker>::BeforeBuffer),
                    // TODO: putting it here means we might miss entities that are spawned and despawned within the send_interval? bug or feature?
                    //  be careful that newly_connected_client is cleared every send_interval, not every frame.
                    send_entity_spawn
                        .in_set(InternalReplicationSet::<ServerMarker>::BufferEntityUpdates),
                    send_entity_despawn
                        .in_set(InternalReplicationSet::<ServerMarker>::BufferDespawnsAndRemovals),
                    (handle_replicating_add, handle_replication_target_update)
                        .in_set(InternalReplicationSet::<ServerMarker>::AfterBuffer),
                ),
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

    /// Component that indicates which clients should predict and interpolate the entity
    #[derive(Component, Default, Clone, Debug, PartialEq, Reflect)]
    pub struct SyncTarget {
        /// Which clients should predict this entity (unused for client to server replication)
        pub prediction: NetworkTarget,
        /// Which clients should interpolate this entity (unused for client to server replication)
        pub interpolation: NetworkTarget,
    }

    /// Component storing metadata about which clients have control over the entity
    ///
    /// This is only used for server to client replication.
    #[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
    pub struct ControlledBy {
        /// Which client(s) control this entity?
        pub target: NetworkTarget,
    }

    impl ControlledBy {
        /// Returns true if the entity is controlled by the specified client
        pub fn targets(&self, client_id: &ClientId) -> bool {
            self.target.targets(client_id)
        }
    }

    /// Component to have more fine-grained control over the visibility of an entity
    /// (which clients do we replicate this entity to?)
    ///
    /// This has no effect for client to server replication.
    #[derive(Component, Clone, Debug, PartialEq, Reflect)]
    pub struct Visibility {
        /// Control if we do fine-grained or coarse-grained visibility
        mode: VisibilityMode,
        // TODO: should we store the visibility cache here if visibility_mode = InterestManagement?
    }

    /// Bundle that indicates how an entity should be replicated. Add this to an entity to start replicating
    /// it to remote peers.
    ///
    /// ```rust
    /// use bevy::prelude::*;
    /// use lightyear::prelude::*;
    /// use lightyear::prelude::server::*;
    ///
    /// let mut world = World::default();
    /// world.spawn(Replicate::default());
    /// ```
    ///
    /// The bundle is composed of several components:
    /// - [`ReplicationTarget`] to specify which clients should receive the entity
    /// - [`SyncTarget`] to specify which clients should predict/interpolate the entity
    /// - [`ControlledBy`] to specify which client controls the entity
    /// - [`VisibilityMode`] to specify if we should replicate the entity to all clients in the
    /// replication target, or if we should apply interest management logic to determine which clients
    /// - [`ReplicationGroup`] to group entities together for replication. Entities in the same group
    /// will be sent together in the same message.
    /// - [`ReplicateHierarchy`] to specify how the hierarchy of the entity should be replicated
    ///
    /// Some of the components can be updated at runtime even after the entity has been replicated.
    /// For example you can update the [`ReplicationTarget`] to change which clients should receive the entity.
    #[derive(Bundle, Clone, Default, PartialEq, Debug, Reflect)]
    pub struct Replicate {
        /// Which clients should this entity be replicated to?
        pub target: ReplicationTarget,
        /// Which clients should predict/interpolate the entity?
        pub sync: SyncTarget,
        /// Which client(s) control this entity?
        pub controlled_by: ControlledBy,
        /// How do we control the visibility of the entity?
        pub visibility: VisibilityMode,
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

    /// In HostServer mode, we will add the Predicted/Interpolated components to the server entities
    /// So that client code can still query for them
    fn add_prediction_interpolation_components(
        mut commands: Commands,
        query: Query<(
            Entity,
            Ref<ReplicationTarget>,
            &SyncTarget,
            Option<&PrePredicted>,
        )>,
        connection: Res<ClientConnection>,
    ) {
        let local_client = connection.id();
        for (entity, replication_target, sync_target, pre_predicted) in query.iter() {
            if (replication_target.is_changed()) && replication_target.target.targets(&local_client)
            {
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
                if sync_target.prediction.targets(&local_client) {
                    commands.entity(entity).insert(Predicted {
                        confirmed_entity: Some(entity),
                    });
                }
                if sync_target.interpolation.targets(&local_client) {
                    commands.entity(entity).insert(Interpolated {
                        confirmed_entity: entity,
                    });
                }
            }
        }
    }

    // TODO: replace this with observers
    /// Metadata that holds Replicate-information from the previous send_interval's replication.
    /// - when the entity gets despawned, we will use this to know how to replicate the despawn
    /// - when the replicate metadata changes, we will use this to compute diffs
    #[derive(PartialEq, Debug)]
    pub(crate) struct ReplicateCache {
        pub(crate) replication_target: NetworkTarget,
        pub(crate) replication_group: ReplicationGroup,
        pub(crate) visibility_mode: VisibilityMode,
        /// If mode = Room, the list of clients that could see the entity
        pub(crate) replication_clients_cache: Vec<ClientId>,
    }

    /// For every entity that removes their ReplicationTarget component but are not despawned, remove the component
    /// from our replicate cache (so that the entity's despawns are no longer replicated)
    pub(crate) fn handle_replicating_remove(
        mut sender: ResMut<ConnectionManager>,
        mut query: RemovedComponents<Replicating>,
        entity_check: &Entities,
    ) {
        for entity in query.read() {
            // only do this for entities that still exist
            if entity_check.contains(entity) {
                debug!("handling replicating component remove (delete from replicate cache)");
                sender.replicate_component_cache.remove(&entity);
                // TODO: should we also remove the replicate-visibility? or should we keep it?
                // commands.entity(entity).remove::<ReplicateVisibility>();
            }
        }
    }

    /// This system does all the additional bookkeeping required after [`Replicating`] has been added:
    /// - adds DespawnTracker to each entity that was ever replicated, so that we can track when they are despawned
    /// (we have a distinction between removing Replicating, which just stops replication; and despawning the entity)
    /// - adds ReplicateCache for that entity so that when it's removed, we can know how to replicate the despawn
    /// - adds the ReplicateVisibility component if needed
    pub(crate) fn handle_replicating_add(
        mut sender: ResMut<ConnectionManager>,
        mut commands: Commands,
        // We use `(With<Replicating>, Without<DespawnTracker>)` as an optimization instead of
        // `Added<Replicating>`
        query: Query<
            (
                Entity,
                &ReplicationTarget,
                &ReplicationGroup,
                &VisibilityMode,
            ),
            (With<Replicating>, Without<DespawnTracker>),
        >,
    ) {
        for (entity, replication_target, group, visibility_mode) in query.iter() {
            debug!("Replicate component was added for entity {entity:?}");
            commands.entity(entity).insert(DespawnTracker);
            let despawn_metadata = ReplicateCache {
                replication_target: replication_target.target.clone(),
                replication_group: *group,
                visibility_mode: *visibility_mode,
                replication_clients_cache: vec![],
            };
            sender
                .replicate_component_cache
                .insert(entity, despawn_metadata);
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
            &SyncTarget,
            &ReplicationGroup,
            &ControlledBy,
            Option<&TargetEntity>,
            Option<&ReplicateVisibility>,
        )>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        // Replicate to already connected clients (replicate only new entities)
        query.iter().for_each(|(entity, replication_target, sync_target, group, controlled_by, target_entity, visibility )| {
            let target = match visibility {
                // for room mode, no need to handle newly-connected clients specially; they just need
                // to be added to the correct room
                Some(visibility) => {
                    visibility.clients_cache
                        .iter()
                        .filter_map(|(client_id, visibility)| {
                            if replication_target.target.targets(client_id) {
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
                            None
                        }).collect()
                }
                None => {
                    let mut target = NetworkTarget::None;
                    // only try to replicate if the replicate component was just added
                    if replication_target.is_added() {
                        trace!(?entity, "send entity spawn");
                        target = replication_target.target.clone();
                    } else if replication_target.is_changed() {
                        target = replication_target.target.clone();
                        if let Some(cached_replicate) = sender.replicate_component_cache.get(&entity) {
                            // do not re-send a spawn message to the clients for which we already have
                            // replicated the entity
                            target.exclude(&cached_replicate.replication_target)
                        }
                    }

                    // also replicate to the newly connected clients that match the target
                    let new_connected_clients = sender.new_connected_clients();
                    if !new_connected_clients.is_empty() {
                        // replicate to the newly connected clients that match our target
                        let mut new_connected_target = NetworkTarget::Only(new_connected_clients);
                        new_connected_target.intersection(&replication_target.target);
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
            //  (i.e. before we received an ack?)
            let _ = sender.apply_replication(target).try_for_each(|client_id| {
                // let the client know that this entity is controlled by them
                if controlled_by.targets(&client_id) {
                    sender.prepare_typed_component_insert(entity, group_id, client_id, component_registry.as_ref(), &Controlled)?;
                }
                // if we need to do prediction/interpolation, send a marker component to indicate that to the client
                if sync_target.prediction.targets(&client_id) {
                    // TODO: the serialized data is always the same; cache it somehow?
                    sender.prepare_typed_component_insert(
                        entity,
                        group_id,
                        client_id,
                        component_registry.as_ref(),
                        &ShouldBePredicted,
                    )?;
                }
                if sync_target.interpolation.targets(&client_id) {
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
                sender.connection_mut(client_id)?.replication_sender.update_base_priority(group_id, group.priority());
                Ok(())
            }).inspect_err(|e: &anyhow::Error| {
                error!("error sending entity spawn: {:?}", e);
            });

        });
    }

    /// Send entity despawn is:
    /// 1) the client lost visibility of the entity
    /// 2) the replication target was updated and the client is no longer in the ReplicationTarget
    /// 3) the entity was despawned
    pub(crate) fn send_entity_despawn(
        query: Query<(
            Entity,
            Ref<ReplicationTarget>,
            &ReplicationGroup,
            Option<&ReplicateVisibility>,
        )>,
        // TODO: ideally we want to send despawns for entities that still had REPLICATE at the time of despawn
        //  not just entities that had despawn tracker once
        mut despawn_removed: RemovedComponents<DespawnTracker>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        // 1. Send entity-despawn for entities where clients lost visibility
        query
            .iter()
            .for_each(|(entity, replication_target, group, visibility)| {
                let mut target: NetworkTarget = match visibility {
                    Some(visibility) => {
                        // send despawn for clients that lost visibility
                        visibility
                            .clients_cache
                            .iter()
                            .filter_map(|(client_id, visibility)| {
                                if replication_target.target.targets(client_id)
                                    && matches!(visibility, ClientVisibility::Lost) {
                                    debug!(
                                    "sending entity despawn for entity: {:?} because ClientVisibility::Lost",
                                    entity
                                    );
                                    return Some(*client_id);
                                }
                                None
                            }).collect()
                    }
                    None => {
                        NetworkTarget::None
                    }
                };
                // 2. if the replication target changed, find the clients that were removed in the new replication target
                if replication_target.is_changed() && !replication_target.is_added() {
                    if let Some(cache) = sender.replicate_component_cache.get_mut(&entity) {
                        let mut new_despawn = cache.replication_target.clone();
                        new_despawn.exclude(&replication_target.target);
                        target.union(&new_despawn);
                    }
                }
                if !target.is_empty() {
                    let _ = sender
                        .prepare_entity_despawn(
                            entity,
                            group,
                            target
                        )
                        .inspect_err(|e| {
                            error!("error sending entity despawn: {:?}", e);
                        });
                }
            });

        // 2. Despawn entities when the entity gets despawned on local world
        for entity in despawn_removed.read() {
            trace!("DespawnTracker component got removed, preparing entity despawn message!");
            // TODO: we still don't want to replicate the despawn if the entity was not in the same room as the client!
            // only replicate the despawn if the entity still had a Replicate component
            if let Some(replicate_cache) = sender.replicate_component_cache.remove(&entity) {
                // TODO: DO NOT SEND ENTITY DESPAWN TO THE CLIENT WHO JUST DISCONNECTED!
                let mut network_target = replicate_cache.replication_target;

                // TODO: for this to work properly, we need the replicate stored in `sender.get_mut_replicate_component_cache()`
                //  to be updated for every replication change! Wait for observers instead.
                //  How did it work on the `main` branch? was there something else making it work? Maybe the
                //  update replicate ran before
                if replicate_cache.visibility_mode == VisibilityMode::InterestManagement {
                    // if the mode was room, only replicate the despawn to clients that were in the same room
                    network_target.intersection(&NetworkTarget::Only(
                        replicate_cache.replication_clients_cache,
                    ));
                }
                trace!(?entity, ?network_target, "send entity despawn");
                let _ = sender
                    .prepare_entity_despawn(
                        entity,
                        &replicate_cache.replication_group,
                        network_target,
                    )
                    // TODO: bubble up errors to user via ConnectionEvents?
                    .inspect_err(|e| {
                        error!("error sending entity despawn: {:?}", e);
                    });
            }
        }
    }

    // TODO: if replication target changed and we are replicating to client 1,
    //  we need to also send component inserts to client 1!
    /// This system sends updates for all components that were added or changed
    /// Sends both ComponentInsert for newly added components
    /// and ComponentUpdates otherwise
    ///
    /// Updates are sent only for any components that were changed since the most recent of:
    /// - last time we sent an action for that group
    /// - last time we sent an update for that group which got acked.
    /// (currently we only check for the second condition, which is enough but less efficient)
    ///
    /// NOTE: cannot use ConnectEvents because they are reset every frame
    pub(crate) fn send_component_update<C: Component>(
        registry: Res<ComponentRegistry>,
        query: Query<
            (
                Entity,
                Ref<C>,
                Ref<ReplicationTarget>,
                Option<&SyncTarget>,
                &ReplicationGroup,
                Option<&ReplicateVisibility>,
                Has<DisabledComponent<C>>,
                Has<ReplicateOnceComponent<C>>,
                Option<&OverrideTargetComponent<C>>,
            ),
            With<Replicating>,
        >,
        system_bevy_ticks: SystemChangeTick,
        mut sender: ResMut<ConnectionManager>,
    ) {
        let kind = registry.net_id::<C>();
        query
            .iter()
            .for_each(|(entity, component, replication_target, sync_target, group,  visibility, disabled, replicate_once, override_target)| {
                // do not replicate components that are disabled
                if disabled {
                    return;
                }
                // use the overriden target if present
                let target = override_target.map_or(&replication_target.target, |override_target| &override_target.target);
                let (insert_target, update_target): (NetworkTarget, NetworkTarget) = match visibility {
                    Some(visibility) => {
                        let mut insert_clients = vec![];
                        let mut update_clients = vec![];
                        visibility
                            .clients_cache
                            .iter()
                            .for_each(|(client_id, visibility)| {
                                if target.targets(client_id) {
                                    match visibility {
                                        ClientVisibility::Gained => {
                                            insert_clients.push(*client_id);
                                        }
                                        ClientVisibility::Lost => {}
                                        ClientVisibility::Maintained => {
                                            // send a component_insert for components that were newly added
                                            if component.is_added() {
                                                insert_clients.push(*client_id);
                                            } else {
                                                // for components that were not newly added, only send as updates
                                                if replicate_once {
                                                    // we can exit the function immediately because we know we don't want to replicate
                                                    // to any client
                                                    return;
                                                }
                                                update_clients.push(*client_id);
                                            }
                                        }
                                    }
                                }
                            });
                        (NetworkTarget::from(insert_clients), NetworkTarget::from(update_clients))
                    }
                    None => {
                        let (mut insert_target, mut update_target) =
                            (NetworkTarget::None, NetworkTarget::None);

                        // send a component_insert for components that were newly added
                        // or if replicate was newly added.
                        // TODO: ideally what we should be checking is: is the component newly added
                        //  for the client we are sending to?
                        //  Otherwise another solution would be to also insert the component on ComponentUpdate if it's missing
                        //  Or should we just have ComponentInsert and ComponentUpdate be the same thing? Or we check
                        //  on the receiver's entity world mut to know if we emit a ComponentInsert or a ComponentUpdate?
                        if component.is_added() || replication_target.is_added() {
                            trace!("component is added or replication_target is added");
                            insert_target.union(target);
                        } else {
                            // do not send updates for these components, only inserts/removes
                            if replicate_once {
                                trace!(?entity,
                                "not replicating updates for {:?} because it is marked as replicate_once",
                                kind
                            );
                                return;
                            }
                            // otherwise send an update for all components that changed since the
                            // last update we have ack-ed
                            update_target.union(target);
                        }

                        let new_connected_clients = sender.new_connected_clients();
                        // replicate all components to newly connected clients
                        if !new_connected_clients.is_empty() {
                            // replicate to the newly connected clients that match our target
                            let mut new_connected_target = NetworkTarget::Only(new_connected_clients);
                            new_connected_target.intersection(target);
                            debug!(?entity, target = ?new_connected_target, "Replicate to newly connected clients");
                            update_target.union(&new_connected_target);
                        }
                        (insert_target, update_target)
                    }
                };
                if !insert_target.is_empty() || !update_target.is_empty() {
                    // serialize component
                    let writer = sender.writer();
                    let raw_data = registry
                        .serialize(component.as_ref(), writer)
                        .expect("Could not serialize component");

                    if !insert_target.is_empty() {
                        let _ = sender
                            .prepare_component_insert(
                                entity,
                                kind,
                                // TODO: avoid the clone by using Arc<u8>?
                                raw_data.clone(),
                                &registry,
                                replication_target.as_ref(),
                                sync_target.map(|sync_target| &sync_target.prediction),
                                group,
                                insert_target
                            )
                            .inspect_err(|e| {
                                error!("error sending component insert: {:?}", e);
                            });
                    }
                    if !update_target.is_empty() {
                        let _ = sender
                            .prepare_component_update(
                                entity,
                                kind,
                                raw_data,
                                group,
                                update_target,
                                component.last_changed(),
                                system_bevy_ticks.this_run(),
                            )
                            .inspect_err(|e| {
                                error!("error sending component update: {:?}", e);
                            });
                    }
                }
            });
    }

    /// This system sends updates for all components that were removed
    pub(crate) fn send_component_removed<C: Component>(
        registry: Res<ComponentRegistry>,
        // only remove the component for entities that are being actively replicated
        query: Query<
            (
                &ReplicationTarget,
                &ReplicationGroup,
                Option<&ReplicateVisibility>,
                Has<DisabledComponent<C>>,
                Option<&OverrideTargetComponent<C>>,
            ),
            With<Replicating>,
        >,
        mut removed: RemovedComponents<C>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        let kind = registry.net_id::<C>();
        removed.read().for_each(|entity| {
            if let Ok((replication_target, group, visibility, disabled, override_target)) =
                query.get(entity)
            {
                // do not replicate components that are disabled
                if disabled {
                    return;
                }
                // use the overriden target if present
                let base_target = override_target
                    .map_or(&replication_target.target, |override_target| {
                        &override_target.target
                    });
                let target = match visibility {
                    Some(visibility) => {
                        visibility
                            .clients_cache
                            .iter()
                            .filter_map(|(client_id, visibility)| {
                                if base_target.targets(client_id) {
                                    // TODO: maybe send no matter the vis?
                                    if matches!(visibility, ClientVisibility::Maintained) {
                                        // TODO: USE THE CUSTOM REPLICATE TARGET FOR THIS COMPONENT IF PRESENT!
                                        return Some(*client_id);
                                    }
                                };
                                None
                            })
                            .collect()
                    }
                    None => {
                        trace!("sending component remove!");
                        // TODO: USE THE CUSTOM REPLICATE TARGET FOR THIS COMPONENT IF PRESENT!
                        base_target.clone()
                    }
                };
                if target.is_empty() {
                    return;
                }
                let group_id = group.group_id(Some(entity));
                debug!(?entity, ?kind, "Sending RemoveComponent");
                let _ = sender.prepare_component_remove(entity, kind, group, target);
            }
        })
    }

    /// Update the replication_target in the cache when the ReplicationTarget component changes
    pub(crate) fn handle_replication_target_update(
        mut sender: ResMut<ConnectionManager>,
        target_query: Query<
            (Entity, Ref<ReplicationTarget>),
            (
                Changed<ReplicationTarget>,
                With<Replicating>,
                With<DespawnTracker>,
            ),
        >,
    ) {
        for (entity, replication_target) in target_query.iter() {
            if replication_target.is_changed() && !replication_target.is_added() {
                if let Some(replicate_cache) = sender.replicate_component_cache.get_mut(&entity) {
                    replicate_cache.replication_target = replication_target.target.clone();
                }
            }
        }
    }

    pub(crate) fn register_replicate_component_send<C: Component>(app: &mut App) {
        app.add_systems(
            PostUpdate,
            (
                // NOTE: we need to run `send_component_removed` once per frame (and not once per send_interval)
                //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
                //  It is ok to run it every frame because it creates at most one message per despawn
                send_component_removed::<C>
                    .in_set(InternalReplicationSet::<ServerMarker>::BufferDespawnsAndRemovals),
                // NOTE: we run this system once every `send_interval` because we don't want to send too many Update messages
                //  and use up all the bandwidth
                send_component_update::<C>
                    .in_set(InternalReplicationSet::<ServerMarker>::BufferComponentUpdates),
            ),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::client::events::ComponentUpdateEvent;
        use crate::prelude::client::Confirmed;
        use crate::prelude::server::{ControlledBy, Replicate, VisibilityManager};
        use crate::prelude::{client, server, Replicated};
        use crate::server::replication::send::SyncTarget;
        use crate::shared::replication::components::Controlled;
        use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
        use crate::tests::protocol::*;
        use crate::tests::stepper::{BevyStepper, Step, TEST_CLIENT_ID};
        use bevy::prelude::{default, EventReader, Resource, Update};

        // TODO: test entity spawn newly connected client

        #[test]
        fn test_entity_spawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper.server_app.world.spawn_empty().id();
            stepper.frame_step();
            stepper.frame_step();
            // check that entity wasn't spawned
            assert!(stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .is_none());

            // add replicate
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(server::Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        interpolation: NetworkTarget::All,
                    },
                    controlled_by: ControlledBy {
                        target: NetworkTarget::All,
                    },
                    ..default()
                });

            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that prediction, interpolation, controlled was handled correctly
            let confirmed = stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Confirmed>()
                .expect("Confirmed component missing");
            assert!(confirmed.predicted.is_some());
            assert!(confirmed.interpolated.is_some());
            assert!(stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Controlled>()
                .is_some());
        }

        #[test]
        fn test_entity_spawn_visibility() {
            let mut stepper = MultiBevyStepper::default();

            // spawn an entity on server with visibility::InterestManagement
            let server_entity = stepper
                .server_app
                .world
                .spawn(server::Replicate {
                    visibility: VisibilityMode::InterestManagement,
                    ..default()
                })
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that entity wasn't spawned
            assert!(stepper
                .client_app_1
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .is_none());
            // make entity visible
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity);
            stepper.frame_step();
            stepper.frame_step();

            // check that entity was spawned
            let client_entity = *stepper
                .client_app_1
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the entity was not spawned on the other client
            assert!(stepper
                .client_app_2
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .is_none());
        }

        #[test]
        fn test_entity_spawn_preexisting_target() {
            let mut stepper = BevyStepper::default();

            let client_entity = stepper.client_app.world.spawn_empty().id();
            stepper.frame_step();
            let server_entity = stepper
                .server_app
                .world
                .spawn((
                    server::Replicate::default(),
                    TargetEntity::Preexisting(client_entity),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was replicated on the client entity
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .resource::<client::ConnectionManager>()
                    .replication_receiver
                    .remote_entity_map
                    .get_local(server_entity)
                    .unwrap(),
                &client_entity
            );
            assert!(stepper
                .client_app
                .world
                .get::<Replicated>(client_entity)
                .is_some());
            assert_eq!(stepper.client_app.world.entities().len(), 1);
        }

        /// Check that if we change the replication target on an entity that already has one
        /// we spawn the entity for new clients
        #[test]
        fn test_entity_spawn_replication_target_update() {
            let mut stepper = MultiBevyStepper::default();

            // spawn an entity on server to client 1
            let server_entity = stepper
                .server_app
                .world
                .spawn(server::Replicate {
                    target: ReplicationTarget {
                        target: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID_1)),
                    },
                    ..default()
                })
                .id();
            stepper.frame_step();
            stepper.frame_step();

            let client_entity_1 = *stepper
                .client_app_1
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client 1");

            // update the replication target
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(ReplicationTarget {
                    target: NetworkTarget::All,
                });
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity gets replicated to the other client
            stepper
                .client_app_2
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client 2");
            // TODO: check that client 1 did not receive another entity-spawn message
        }

        #[test]
        fn test_entity_despawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn(server::Replicate::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // despawn
            stepper.server_app.world.despawn(server_entity);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned
            assert!(stepper.client_app.world.get_entity(client_entity).is_none());
        }

        /// Check that if interest management is used, a client losing visibility of an entity
        /// will cause the server to send a despawn-entity message to the client
        #[test]
        fn test_entity_despawn_lose_visibility() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn(server::Replicate {
                    visibility: VisibilityMode::InterestManagement,
                    ..default()
                })
                .id();
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);

            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // lose visibility
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .lose_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned
            assert!(stepper.client_app.world.get_entity(client_entity).is_none());
        }

        /// Test that if an entity with visibility is despawned, the despawn-message is not sent
        /// to other clients who do not have visibility of the entity
        #[test]
        fn test_entity_despawn_non_visible() {
            let mut stepper = MultiBevyStepper::default();

            // spawn one entity replicated to each client
            // they will share the same replication group id, so that each client's ReplicationReceiver
            // can read the replication messages of the other client
            let server_entity_1 = stepper
                .server_app
                .world
                .spawn(server::Replicate {
                    visibility: VisibilityMode::InterestManagement,
                    group: ReplicationGroup::new_id(1),
                    ..default()
                })
                .id();
            let server_entity_2 = stepper
                .server_app
                .world
                .spawn(server::Replicate {
                    visibility: VisibilityMode::InterestManagement,
                    group: ReplicationGroup::new_id(1),
                    ..default()
                })
                .id();
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity_1)
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_2), server_entity_2);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned on each client
            let client_entity_1 = *stepper
                .client_app_1
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity_1)
                .expect("entity was not replicated to client 1");
            let client_entity_2 = *stepper
                .client_app_2
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity_2)
                .expect("entity was not replicated to client 2");

            // update the entity_map on client 2 to re-use the same server entity as client 1
            // so that replication messages for server_entity_1 could also be read by client 2
            stepper
                .client_app_2
                .world
                .resource_mut::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .insert(server_entity_1, client_entity_2);

            // despawn the server_entity_1
            stepper.server_app.world.despawn(server_entity_1);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned on client 1
            assert!(stepper
                .client_app_1
                .world
                .get_entity(client_entity_1)
                .is_none());

            // check that the entity still exists on client 2
            assert!(stepper
                .client_app_2
                .world
                .get_entity(client_entity_2)
                .is_some());
        }

        /// Check that if we change the replication target on an entity that already has one
        /// we despawn the entity for new clients
        #[test]
        fn test_entity_despawn_replication_target_update() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server to client 1
            let server_entity = stepper
                .server_app
                .world
                .spawn(server::Replicate {
                    target: ReplicationTarget {
                        target: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID)),
                    },
                    ..default()
                })
                .id();
            stepper.frame_step();
            stepper.frame_step();

            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // update the replication target
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(ReplicationTarget {
                    target: NetworkTarget::None,
                });
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned
            assert!(stepper.client_app.world.get_entity(client_entity).is_none());
        }

        #[test]
        fn test_component_insert() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn(server::Replicate::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(Component1(1.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );
        }

        #[test]
        fn test_component_insert_visibility_maintained() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn(Replicate {
                    visibility: VisibilityMode::InterestManagement,
                    ..default()
                })
                .id();
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(Component1(1.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );
        }

        #[test]
        fn test_component_insert_visibility_gained() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn(Replicate {
                    visibility: VisibilityMode::InterestManagement,
                    ..default()
                })
                .id();

            stepper.frame_step();
            stepper.frame_step();

            // add component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(Component1(1.0));
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
            stepper.frame_step();
            stepper.frame_step();

            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );
        }

        #[test]
        fn test_component_insert_disabled() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper.server_app.world.spawn(Replicate::default()).id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert((Component1(1.0), DisabledComponent::<Component1>::default()));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was not replicated
            assert!(stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .is_none());
        }

        #[test]
        fn test_component_override_target() {
            let mut stepper = MultiBevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn((
                    Replicate::default(),
                    Component1(1.0),
                    OverrideTargetComponent::<Component1>::new(NetworkTarget::Single(
                        ClientId::Netcode(TEST_CLIENT_ID_1),
                    )),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity_1 = *stepper
                .client_app_1
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_entity_2 = *stepper
                .client_app_2
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // check that the component was replicated to client 1 only
            assert_eq!(
                stepper
                    .client_app_1
                    .world
                    .entity(client_entity_1)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );
            assert!(stepper
                .client_app_2
                .world
                .entity(client_entity_2)
                .get::<Component1>()
                .is_none());
        }

        /// Check that override target works even if the entity uses interest management
        /// We still use visibility, but we use `override_target` instead of `replication_target`
        #[test]
        fn test_component_override_target_visibility() {
            let mut stepper = MultiBevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn((
                    Replicate {
                        // target is both
                        visibility: VisibilityMode::InterestManagement,
                        ..default()
                    },
                    Component1(1.0),
                    // override target is only client 1
                    OverrideTargetComponent::<Component1>::new(NetworkTarget::Single(
                        ClientId::Netcode(TEST_CLIENT_ID_1),
                    )),
                ))
                .id();
            // entity is visible to both
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity)
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_2), server_entity);
            stepper.frame_step();
            stepper.frame_step();
            let client_entity_1 = *stepper
                .client_app_1
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_entity_2 = *stepper
                .client_app_2
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // check that the component was replicated to client 1 only
            assert_eq!(
                stepper
                    .client_app_1
                    .world
                    .entity(client_entity_1)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );
            assert!(stepper
                .client_app_2
                .world
                .entity(client_entity_2)
                .get::<Component1>()
                .is_none());
        }

        #[test]
        fn test_component_update() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn((Replicate::default(), Component1(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(Component1(2.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(2.0)
            );
        }

        /// Check that updates are not sent if the `ReplicationTarget` component gets removed.
        /// Check that updates are resumed when the `ReplicationTarget` component gets re-added.
        #[test]
        fn test_component_update_replication_target_removed() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn((Replicate::default(), Component1(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // remove the replication_target component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(Component1(2.0))
                .remove::<ReplicationTarget>();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity still exists on the client, but that the component was not updated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );

            // re-add the replication_target component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(ReplicationTarget::default());
            stepper.frame_step();
            stepper.frame_step();
            // check that the component gets updated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(2.0)
            );
        }

        #[test]
        fn test_component_update_disabled() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn((Replicate::default(), Component1(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert((Component1(2.0), DisabledComponent::<Component1>::default()));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was not updated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );
        }

        #[test]
        fn test_component_update_replicate_once() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn((
                    Replicate::default(),
                    Component1(1.0),
                    ReplicateOnceComponent::<Component1>::default(),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );

            // update component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .insert(Component1(2.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was not updated
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );
        }

        #[test]
        fn test_component_remove() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world
                .spawn((Replicate::default(), Component1(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .entity(client_entity)
                    .get::<Component1>()
                    .expect("component missing"),
                &Component1(1.0)
            );

            // remove component
            stepper
                .server_app
                .world
                .entity_mut(server_entity)
                .remove::<Component1>();
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert!(stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .is_none());
        }

        #[test]
        fn test_replicating_add() {
            let mut stepper = BevyStepper::default();

            let server_entity = stepper.server_app.world.spawn(Replicate::default()).id();
            stepper.frame_step();

            // check that a DespawnTracker was added
            assert!(stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<DespawnTracker>()
                .is_some());
            // check that a ReplicateCache was added
            assert_eq!(
                stepper
                    .server_app
                    .world
                    .resource::<server::ConnectionManager>()
                    .replicate_component_cache
                    .get(&server_entity)
                    .expect("ReplicateCache missing"),
                &ReplicateCache {
                    replication_target: NetworkTarget::All,
                    replication_group: ReplicationGroup::new_from_entity(),
                    visibility_mode: VisibilityMode::All,
                    replication_clients_cache: vec![],
                }
            );
        }

        /// Check that if we switch the visibility mode, the entity gets spawned
        /// to the clients that now have visibility
        #[test]
        fn test_change_visibility_mode_spawn() {
            let mut stepper = BevyStepper::default();

            let server_entity = stepper
                .server_app
                .world
                .spawn(Replicate {
                    target: ReplicationTarget {
                        target: NetworkTarget::None,
                    },
                    ..default()
                })
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // set visibility to interest management
            stepper.server_app.world.entity_mut(server_entity).insert((
                VisibilityMode::InterestManagement,
                ReplicationTarget {
                    target: NetworkTarget::All,
                },
            ));
            stepper
                .server_app
                .world
                .resource_mut::<VisibilityManager>()
                .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);

            stepper.frame_step();
            stepper.frame_step();
            stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
        }

        /// Check if we send an update with a component that is already equal to the component on the remote,
        /// then we do not apply the update to the remote (to avoid triggering change detection)
        #[test]
        fn test_equal_update_does_not_trigger_change_detection() {
            let mut stepper = BevyStepper::default();

            stepper.client_app.add_systems(
                Update,
                |mut events: EventReader<ComponentUpdateEvent<Component1>>| {
                    if let Some(event) = events.read().next() {
                        panic!(
                            "ComponentUpdateEvent received for entity: {:?}",
                            event.entity()
                        );
                    }
                },
            );

            // spawn an entity on server
            let server_entity = stepper.server_app.world.spawn(Component1(1.0)).id();
            // spawn an entity on the client with the component value
            let client_entity = stepper.client_app.world.spawn(Component1(1.0)).id();

            // add replication with a pre-existing target
            stepper.server_app.world.entity_mut(server_entity).insert((
                Replicate::default(),
                TargetEntity::Preexisting(client_entity),
            ));

            // check that we did not receive an ComponentUpdateEvent because the component was already equal
            // to the replicated value
            stepper.frame_step();
            stepper.frame_step();
        }

        #[derive(Resource, Default)]
        struct Counter(u32);

        /// Check if we send an update with a component that is not equal to the component on the remote,
        /// then we apply the update to the remote (so we emit a ComponentUpdateEvent)
        #[test]
        fn test_not_equal_update_does_not_trigger_change_detection() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper.server_app.world.spawn(Component1(2.0)).id();
            // spawn an entity on the client with the component value
            let client_entity = stepper.client_app.world.spawn(Component1(1.0)).id();

            stepper.client_app.init_resource::<Counter>();
            stepper.client_app.add_systems(
                Update,
                move |mut events: EventReader<ComponentUpdateEvent<Component1>>,
                      mut counter: ResMut<Counter>| {
                    for events in events.read() {
                        counter.0 += 1;
                        assert_eq!(events.entity(), client_entity);
                    }
                },
            );

            // add replication with a pre-existing target
            stepper.server_app.world.entity_mut(server_entity).insert((
                Replicate::default(),
                TargetEntity::Preexisting(client_entity),
            ));

            // check that we did receive an ComponentUpdateEvent
            stepper.frame_step();
            stepper.frame_step();
            assert_eq!(
                stepper
                    .client_app
                    .world
                    .get_resource::<Counter>()
                    .unwrap()
                    .0,
                1
            );
        }
    }
}

pub(crate) mod commands {
    use crate::server::connection::ConnectionManager;
    use crate::shared::replication::ReplicationSend;
    use bevy::ecs::system::{Command, EntityCommands};
    use bevy::prelude::{Entity, World};

    fn despawn_without_replication(entity: Entity, world: &mut World) {
        let mut sender = world.resource_mut::<ConnectionManager>();
        // remove the entity from the cache of entities that are being replicated
        // so that if it gets despawned, the despawn won't be replicated
        sender.replicate_component_cache.remove(&entity);
        world.despawn(entity);
    }

    pub trait DespawnReplicationCommandExt {
        /// Despawn the entity and makes sure that the despawn won't be replicated.
        fn despawn_without_replication(&mut self);
    }
    impl DespawnReplicationCommandExt for EntityCommands<'_> {
        fn despawn_without_replication(&mut self) {
            self.add(despawn_without_replication);
        }
    }

    #[cfg(test)]
    mod tests {
        use bevy::utils::Duration;

        use crate::client::sync::SyncConfig;
        use crate::prelude::client::{InterpolationConfig, PredictionConfig};
        use crate::prelude::{server, LinkConditionerConfig, Replicate, SharedConfig, TickConfig};
        use crate::tests::protocol::*;
        use crate::tests::stepper::{BevyStepper, Step};

        use super::*;

        // TODO: simplify tests, we don't need a client-server connection here
        #[test]
        fn test_despawn() {
            let tick_duration = Duration::from_millis(10);
            let frame_duration = Duration::from_millis(10);
            let shared_config = SharedConfig {
                tick: TickConfig::new(tick_duration),
                ..Default::default()
            };
            let link_conditioner = LinkConditionerConfig {
                incoming_latency: Duration::from_millis(0),
                incoming_jitter: Duration::from_millis(0),
                incoming_loss: 0.0,
            };
            let sync_config = SyncConfig::default().speedup_factor(1.0);
            let prediction_config = PredictionConfig::default();
            let interpolation_config = InterpolationConfig::default();
            let mut stepper = BevyStepper::new(
                shared_config,
                sync_config,
                prediction_config,
                interpolation_config,
                link_conditioner,
                frame_duration,
            );
            stepper.init();

            let entity = stepper
                .server_app
                .world
                .spawn((Component1(1.0), Replicate::default()))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            assert!(stepper
                .client_app
                .world
                .query::<&Component1>()
                .get_single(&stepper.client_app.world)
                .is_ok());

            // if we remove the Replicate component, and then despawn the entity
            // the despawn still gets replicated
            stepper
                .server_app
                .world
                .entity_mut(entity)
                .remove::<Replicate>();
            stepper.server_app.world.entity_mut(entity).despawn();
            stepper.frame_step();
            stepper.frame_step();

            assert!(stepper
                .client_app
                .world
                .query::<&Component1>()
                .get_single(&stepper.client_app.world)
                .is_err());

            // spawn a new entity
            let entity = stepper
                .server_app
                .world
                .spawn((Component1(1.0), Replicate::default()))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            assert!(stepper
                .client_app
                .world
                .query::<&Component1>()
                .get_single(&stepper.client_app.world)
                .is_ok());

            // apply the command to remove replicate
            despawn_without_replication(entity, &mut stepper.server_app.world);
            stepper.frame_step();
            stepper.frame_step();
            // now the despawn should not have been replicated
            assert!(stepper
                .client_app
                .world
                .query::<&Component1>()
                .get_single(&stepper.client_app.world)
                .is_ok());
        }
    }
}
