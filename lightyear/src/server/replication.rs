use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::components::Confirmed;
use crate::client::interpolation::Interpolated;
use crate::client::prediction::Predicted;
use crate::connection::client::NetClient;
use crate::prelude::client::ClientConnection;
use crate::prelude::{server::is_started, PrePredicted};
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::prediction::compute_hash;
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, ServerMarker};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ServerReplicationSet {
    // You can use this SystemSet to add Replicate components to entities received from clients (to rebroadcast them to other clients)
    ClientReplication,
}

pub type ReplicationSet = InternalReplicationSet<ServerMarker>;

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
        is_host_server, ClientId, ComponentRegistry, DisabledComponent, NetworkRelevanceMode,
        OverrideTargetComponent, ReplicateHierarchy, ReplicationGroup, ShouldBePredicted,
        TargetEntity, Tick, TickManager, TimeManager,
    };
    use crate::protocol::component::ComponentKind;
    use crate::server::error::ServerError;
    use crate::server::relevance::immediate::{CachedNetworkRelevance, ClientRelevance};
    use crate::shared::replication::archetypes::{
        get_erased_component, ServerReplicatedArchetypes,
    };
    use crate::shared::replication::authority::{AuthorityPeer, HasAuthority};
    use crate::shared::replication::components::{
        Cached, Controlled, Replicating, ReplicationGroupId, ReplicationTarget,
        ShouldBeInterpolated,
    };
    use crate::shared::replication::network_target::NetworkTarget;
    use crate::shared::replication::ReplicationSend;
    use bevy::ecs::component::ComponentTicks;
    use bevy::ecs::system::SystemChangeTick;
    use bevy::ptr::Ptr;

    #[derive(Default)]
    pub struct ServerReplicationSendPlugin {
        pub tick_interval: Duration,
    }

    impl Plugin for ServerReplicationSendPlugin {
        fn build(&self, app: &mut App) {
            let send_interval = app
                .world()
                .resource::<ServerConfig>()
                .replication
                .send_interval;

            app
                // REFLECTION
                .register_type::<Replicate>()
                // PLUGIN
                .add_plugins(ReplicationSendPlugin::<ConnectionManager>::new(
                    self.tick_interval,
                    send_interval,
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
                PostUpdate,
                (
                    // TODO: putting it here means we might miss entities that are spawned and despawned within the send_interval? bug or feature?
                    //  be careful that newly_connected_client is cleared every send_interval, not every frame.
                    replicate
                        .in_set(InternalReplicationSet::<ServerMarker>::BufferEntityUpdates)
                        .in_set(InternalReplicationSet::<ServerMarker>::BufferComponentUpdates),
                    (
                        handle_replication_target_update,
                        buffer_replication_messages,
                    )
                        .in_set(InternalReplicationSet::<ServerMarker>::AfterBuffer),
                ),
            );
            // HOST-SERVER
            app.add_systems(
                PostUpdate,
                add_prediction_interpolation_components
                    // .after(InternalMainSet::<ServerMarker>::SendMessages)
                    .run_if(is_host_server),
            );

            app.observe(replicate_entity_local_despawn);
            app.observe(add_has_authority_component);
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
    #[reflect(Component)]
    pub struct ControlledBy {
        /// Which client(s) control this entity?
        pub target: NetworkTarget,
        /// What happens to the entity if the controlling client disconnects?
        pub lifetime: Lifetime,
    }

    impl ControlledBy {
        /// Returns true if the entity is controlled by the specified client
        pub fn targets(&self, client_id: &ClientId) -> bool {
            self.target.targets(client_id)
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
    pub enum Lifetime {
        #[default]
        /// When the client that controls the entity disconnects, the entity is despawned
        SessionBased,
        /// The entity is not despawned even if the controlling client disconnects
        Persistent,
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
    /// - [`NetworkRelevanceMode`] to specify if we should replicate the entity to all clients in the
    ///   replication target, or if we should apply interest management logic to determine which clients
    /// - [`ReplicationGroup`] to group entities together for replication. Entities in the same group
    ///   will be sent together in the same message.
    /// - [`ReplicateHierarchy`] to specify how the hierarchy of the entity should be replicated
    ///
    /// Some of the components can be updated at runtime even after the entity has been replicated.
    /// For example you can update the [`ReplicationTarget`] to change which clients should receive the entity.
    #[derive(Bundle, Clone, Default, PartialEq, Debug, Reflect)]
    pub struct Replicate {
        /// Which clients should this entity be replicated to?
        pub target: ReplicationTarget,
        // TODO: if AuthorityPeer::Server is added, need to add HasAuthority (via observer)
        /// Who has authority over the entity? i.e. who is in charge of simulating the entity and sending replication updates?
        pub authority: AuthorityPeer,
        /// Which clients should predict/interpolate the entity?
        pub sync: SyncTarget,
        /// How do we control the visibility of the entity?
        pub relevance_mode: NetworkRelevanceMode,
        /// Which client(s) control this entity?
        pub controlled_by: ControlledBy,
        /// The replication group defines how entities are grouped (sent as a single message) for replication.
        ///
        /// After the entity is first replicated, the replication group of the entity should not be modified.
        /// (but more entities can be added to the replication group)
        pub group: ReplicationGroup,
        /// How should the hierarchy of the entity (parents/children) be replicated?
        pub hierarchy: ReplicateHierarchy,
        pub marker: Replicating,
    }

    /// Buffer the replication messages into channels
    fn buffer_replication_messages(
        change_tick: SystemChangeTick,
        mut connection_manager: ResMut<ConnectionManager>,
        tick_manager: Res<TickManager>,
        time_manager: Res<TimeManager>,
    ) {
        connection_manager
            .buffer_replication_messages(
                tick_manager.tick(),
                change_tick.this_run(),
                time_manager.as_ref(),
            )
            .unwrap_or_else(|e| {
                error!("Error preparing replicate send: {}", e);
            });
        // TODO: how to handle this for replication groups that update less frequently?
        //  only component updates should update less frequently, but entity spawns/removals
        //  should be sent with the same frequency!
        // clear the list of newly connected clients
        connection_manager.new_clients.clear();
    }

    /// In HostServer mode, we will add the Predicted/Interpolated components to the server entities
    /// So that client code can still query for them
    fn add_prediction_interpolation_components(
        mut commands: Commands,
        query: Query<(
            Entity,
            Ref<ReplicationTarget>,
            &SyncTarget,
            Option<Ref<ControlledBy>>,
            Option<&PrePredicted>,
        )>,
        connection: Res<ClientConnection>,
    ) {
        let local_client = connection.id();
        for (entity, replication_target, sync_target, controlled_by, pre_predicted) in query.iter()
        {
            // also insert [`Controlled`] on the entity if it's controlled by the local client
            if let Some(controlled_by) = controlled_by {
                if controlled_by.is_changed() && controlled_by.targets(&local_client) {
                    commands
                        .entity(entity)
                        // NOTE: do not replicate this Controlled to other clients, or they will
                        // think they control this entity
                        .insert((Controlled, DisabledComponent::<Controlled>::default()));
                }
            }
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
        pub(crate) network_relevance_mode: NetworkRelevanceMode,
        /// If mode = Room, the list of clients that could see the entity
        pub(crate) replication_clients_cache: Vec<ClientId>,
    }

    /// Keep a cached version of the [`ReplicationTarget`] component so that when it gets updated
    /// we can compute a diff with the previous value.
    ///
    /// This needs to run after we compute the diff, so after the `replicate` system runs
    pub(crate) fn handle_replication_target_update(
        mut commands: Commands,
        mut query: Query<
            (
                Entity,
                &ReplicationTarget,
                Option<&mut Cached<ReplicationTarget>>,
            ),
            Changed<ReplicationTarget>,
        >,
    ) {
        for (entity, replication_target, cached) in query.iter_mut() {
            if let Some(mut cached) = cached {
                cached.value = replication_target.clone();
            } else {
                commands.entity(entity).insert(Cached {
                    value: replication_target.clone(),
                });
            }
        }
    }

    /// Add HasAuthority component to a newly replicated entity if the server has
    /// authority over it
    fn add_has_authority_component(
        // NOTE: we do not trigger on OnMutate, it's only when AuthorityPeer is first
        // added that we check if the server has authority. After that, we should
        // rely only on commands.transfer_authority
        trigger: Trigger<OnAdd, AuthorityPeer>,
        mut commands: Commands,
    ) {
        commands
            .entity(trigger.entity())
            .add(|mut entity_mut: EntityWorldMut| {
                if entity_mut
                    .get::<AuthorityPeer>()
                    .is_some_and(|authority| *authority == AuthorityPeer::Server)
                {
                    entity_mut.insert(HasAuthority);
                }
            });
    }

    pub(crate) fn replicate(
        tick_manager: Res<TickManager>,
        component_registry: Res<ComponentRegistry>,
        mut replicated_archetypes: Local<ServerReplicatedArchetypes>,
        system_ticks: SystemChangeTick,
        mut set: ParamSet<(&World, ResMut<ConnectionManager>)>,
    ) {
        // 1. update the list of replicated archetypes
        replicated_archetypes.update(set.p0(), &component_registry);

        let mut sender = std::mem::take(&mut *set.p1());
        let world = set.p0();

        // 2. go through all the archetypes that should be replicated
        for replicated_archetype in replicated_archetypes.archetypes.iter() {
            // SAFETY: update() makes sure that we have a valid archetype
            let archetype = unsafe {
                world
                    .archetypes()
                    .get(replicated_archetype.id)
                    .unwrap_unchecked()
            };
            let table = unsafe {
                world
                    .storages()
                    .tables
                    .get(archetype.table_id())
                    .unwrap_unchecked()
            };

            // a. add all entity despawns from entities that were despawned locally
            // (done in a separate system)
            // replicate_entity_local_despawn(&mut despawn_removed, &mut set.p1());

            // 3. go through all entities of that archetype
            for entity in archetype.entities() {
                let entity_ref = world.entity(entity.id());
                let group = entity_ref.get::<ReplicationGroup>();

                let group_id = group.map_or(ReplicationGroupId::default(), |g| {
                    g.group_id(Some(entity.id()))
                });
                let priority = group.map_or(1.0, |g| g.priority());
                let cached_replication_target = entity_ref.get::<Cached<ReplicationTarget>>();
                let visibility = entity_ref.get::<CachedNetworkRelevance>();
                let sync_target = entity_ref.get::<SyncTarget>();
                let target_entity = entity_ref.get::<TargetEntity>();
                let controlled_by = entity_ref.get::<ControlledBy>();
                // SAFETY: we know that the entity has the ReplicationTarget component
                // because the archetype is in replicated_archetypes
                let replication_target =
                    unsafe { entity_ref.get::<ReplicationTarget>().unwrap_unchecked() };
                let replication_target_ticks = unsafe {
                    entity_ref
                        .get_change_ticks::<ReplicationTarget>()
                        .unwrap_unchecked()
                };
                let (added_tick, changed_tick) = (
                    replication_target_ticks.added_tick(),
                    replication_target_ticks.last_changed_tick(),
                );
                // entity_ref::get_ref() does not do what we want (https://github.com/bevyengine/bevy/issues/13735)
                // so create the ref manually
                let replication_target = Ref::new(
                    replication_target,
                    &added_tick,
                    &changed_tick,
                    system_ticks.last_run(),
                    system_ticks.this_run(),
                );

                // b. add entity despawns from visibility or target change
                replicate_entity_despawn(
                    entity.id(),
                    group_id,
                    &replication_target,
                    cached_replication_target,
                    visibility,
                    &mut sender,
                );

                // c. add all entity spawns
                replicate_entity_spawn(
                    &component_registry,
                    entity.id(),
                    &replication_target,
                    cached_replication_target,
                    group_id,
                    priority,
                    controlled_by,
                    sync_target,
                    target_entity,
                    visibility,
                    &mut sender,
                    &system_ticks,
                );

                // If the group is not set to send, skip sending updates for this entity
                if group.is_some_and(|g| !g.should_send) {
                    continue;
                }

                // d. all components that were added or changed
                for replicated_component in replicated_archetype.components.iter() {
                    let (data, component_ticks) = unsafe {
                        get_erased_component(
                            table,
                            &world.storages().sparse_sets,
                            entity,
                            replicated_component.storage_type,
                            replicated_component.id,
                        )
                    };
                    let override_target = replicated_component.override_target.and_then(|id| {
                        entity_ref
                            .get_by_id(id)
                            // SAFETY: we know the archetype has the OverrideTarget<C> component
                            // the OverrideTarget<C> component has the same memory layout as NetworkTarget
                            .map(|ptr| unsafe { ptr.deref::<NetworkTarget>() })
                    });

                    replicate_component_updates(
                        tick_manager.tick(),
                        &component_registry,
                        entity.id(),
                        replicated_component.kind,
                        data,
                        component_ticks,
                        &replication_target,
                        sync_target,
                        group_id,
                        visibility,
                        replicated_component.delta_compression,
                        replicated_component.replicate_once,
                        override_target,
                        &system_ticks,
                        &mut sender,
                    );
                }

                // e. add all removed components
            }
        }

        *set.p1() = sender;
    }

    /// Send entity spawn replication messages to clients
    /// Also handles:
    /// - newly_connected_clients should receive the entity spawn message even if the entity was not just spawned
    /// - adds ControlledBy, ShouldBePredicted, ShouldBeInterpolated component
    /// - handles TargetEntity if it's a Preexisting entity
    pub(crate) fn replicate_entity_spawn(
        component_registry: &ComponentRegistry,
        entity: Entity,
        replication_target: &Ref<ReplicationTarget>,
        cached_replication_target: Option<&Cached<ReplicationTarget>>,
        group_id: ReplicationGroupId,
        priority: f32,
        controlled_by: Option<&ControlledBy>,
        sync_target: Option<&SyncTarget>,
        target_entity: Option<&TargetEntity>,
        visibility: Option<&CachedNetworkRelevance>,
        sender: &mut ConnectionManager,
        system_ticks: &SystemChangeTick,
    ) {
        let target = match visibility {
            // for room mode, no need to handle newly-connected clients specially; they just need
            // to be added to the correct room
            Some(visibility) => {
                visibility
                    .clients_cache
                    .iter()
                    .filter_map(|(client_id, visibility)| {
                        if replication_target.target.targets(client_id) {
                            match visibility {
                                ClientRelevance::Gained => {
                                    trace!(
                                        ?entity,
                                        ?client_id,
                                        "send entity spawn to client who just gained visibility"
                                    );
                                    return Some(*client_id);
                                }
                                ClientRelevance::Lost => {}
                                ClientRelevance::Maintained => {
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
                    })
                    .collect()
            }
            None => {
                let mut target = NetworkTarget::None;
                // only try to replicate if the replicate component was just added
                if replication_target.is_added() {
                    trace!(?entity, "send entity spawn");
                    // TODO: avoid this clone!
                    target = replication_target.target.clone();
                } else if replication_target.is_changed() {
                    target = replication_target.target.clone();
                    // if the replication target changed (for example from [1] to [1, 2]), do not replicate again to [1]
                    if let Some(cached_target) = cached_replication_target {
                        // do not re-send a spawn message to the clients for which we already have
                        // replicated the entity
                        target.exclude(&cached_target.value.target)
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
        // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
        //  (i.e. before we received an ack?)
        let _ = sender
            .connected_targets(target)
            .try_for_each(|client_id| {
                // let the client know that this entity is controlled by them
                if controlled_by.is_some_and(|c| c.targets(&client_id)) {
                    sender.prepare_typed_component_insert(
                        entity,
                        group_id,
                        client_id,
                        component_registry,
                        &mut Controlled,
                        system_ticks.this_run(),
                    )?;
                }
                // if we need to do prediction/interpolation, send a marker component to indicate that to the client
                if sync_target.is_some_and(|sync| sync.prediction.targets(&client_id)) {
                    // TODO: the serialized data is always the same; cache it somehow?
                    sender.prepare_typed_component_insert(
                        entity,
                        group_id,
                        client_id,
                        component_registry,
                        &mut ShouldBePredicted,
                        system_ticks.this_run(),
                    )?;
                }
                if sync_target.is_some_and(|sync| sync.interpolation.targets(&client_id)) {
                    sender.prepare_typed_component_insert(
                        entity,
                        group_id,
                        client_id,
                        component_registry,
                        &mut ShouldBeInterpolated,
                        system_ticks.this_run(),
                    )?;
                }

                if let Some(TargetEntity::Preexisting(remote_entity)) = target_entity {
                    sender
                        .connection_mut(client_id)?
                        .replication_sender
                        .prepare_entity_spawn_reuse(entity, group_id, *remote_entity);
                } else {
                    sender
                        .connection_mut(client_id)?
                        .replication_sender
                        .prepare_entity_spawn(entity, group_id);
                }

                // also set the priority for the group when we spawn it
                sender
                    .connection_mut(client_id)?
                    .replication_sender
                    .update_base_priority(group_id, priority);
                Ok(())
            })
            .inspect_err(|e: &ServerError| {
                error!("error sending entity spawn: {:?}", e);
            });
    }

    /// Despawn entities when the entity gets despawned on local world
    pub(crate) fn replicate_entity_local_despawn(
        // we use the removal of ReplicationGroup to detect the despawn
        trigger: Trigger<OnRemove, ReplicationGroup>,
        // only replicate despawns to entities that still had Replicating at the time of their despawn
        query: Query<
            (
                &ReplicationGroup,
                &ReplicationTarget,
                Option<&CachedNetworkRelevance>,
            ),
            With<Replicating>,
        >,
        // TODO: should we use Option<ResMut> so that this observer doesn't trigger
        //  when we are not connected?
        mut sender: ResMut<ConnectionManager>,
    ) {
        let entity = trigger.entity();
        if let Ok((replication_group, network_target, cached_relevance)) = query.get(entity) {
            trace!(?entity, "Replicate entity despawn");
            // only send the despawn to clients who were in the target of the entity
            let mut target = network_target.clone().target;
            // only send the despawn to clients that had visibility of the entity
            if let Some(network_relevance) = cached_relevance {
                // TODO: optimize this in cases like All/None/Single/ExceptSingle
                target.intersection(&NetworkTarget::Only(
                    network_relevance.clients_cache.keys().copied().collect(),
                ))
            }
            trace!(?entity, ?target, "send entity despawn");
            let _ = sender
                .prepare_entity_despawn(entity, replication_group.group_id(Some(entity)), target)
                // TODO: bubble up errors to user via ConnectionEvents?
                .inspect_err(|e| {
                    error!("error sending entity despawn: {:?}", e);
                });
        }
    }

    /// Send entity despawn is:
    /// 1) the client lost visibility of the entity
    /// 2) the replication target was updated and the client is no longer in the ReplicationTarget
    pub(crate) fn replicate_entity_despawn(
        entity: Entity,
        group_id: ReplicationGroupId,
        replication_target: &Ref<ReplicationTarget>,
        cached_replication_target: Option<&Cached<ReplicationTarget>>,
        visibility: Option<&CachedNetworkRelevance>,
        sender: &mut ConnectionManager,
    ) {
        // 1. send despawn for clients that lost visibility
        let mut target: NetworkTarget = match visibility {
            Some(visibility) => {
                visibility
                    .clients_cache
                    .iter()
                    .filter_map(|(client_id, visibility)| {
                        if replication_target.target.targets(client_id)
                            && matches!(visibility, ClientRelevance::Lost) {
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
            if let Some(cached_target) = cached_replication_target {
                // get targets that we had before but not anymore
                let mut new_despawn = cached_target.value.target.clone();
                new_despawn.exclude(&replication_target.target);
                target.union(&new_despawn);
            }
        }
        if !target.is_empty() {
            let _ = sender
                .prepare_entity_despawn(entity, group_id, target)
                .inspect_err(|e| {
                    error!("error sending entity despawn: {:?}", e);
                });
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
    ///   (currently we only check for the second condition, which is enough but less efficient)
    ///
    /// NOTE: cannot use ConnectEvents because they are reset every frame
    pub(crate) fn replicate_component_updates(
        current_tick: Tick,
        component_registry: &ComponentRegistry,
        entity: Entity,
        component_kind: ComponentKind,
        component_data: Ptr,
        component_ticks: ComponentTicks,
        replication_target: &Ref<ReplicationTarget>,
        sync_target: Option<&SyncTarget>,
        group_id: ReplicationGroupId,
        visibility: Option<&CachedNetworkRelevance>,
        delta_compression: bool,
        replicate_once: bool,
        override_target: Option<&NetworkTarget>,
        system_ticks: &SystemChangeTick,
        sender: &mut ConnectionManager,
    ) {
        // TODO: maybe iterate through all the connected clients instead, to avoid allocations?
        // use the overriden target if present
        let target = override_target.map_or(&replication_target.target, |override_target| {
            override_target
        });
        let (insert_target, mut update_target): (NetworkTarget, NetworkTarget) = match visibility {
            Some(visibility) => {
                let mut insert_clients = vec![];
                let mut update_clients = vec![];
                visibility
                    .clients_cache
                    .iter()
                    .for_each(|(client_id, visibility)| {
                        if target.targets(client_id) {
                            match visibility {
                                ClientRelevance::Gained => {
                                    insert_clients.push(*client_id);
                                }
                                ClientRelevance::Lost => {}
                                ClientRelevance::Maintained => {
                                    // send a component_insert for components that were newly added
                                    if component_ticks
                                        .is_added(system_ticks.last_run(), system_ticks.this_run())
                                    {
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
                (
                    NetworkTarget::from(insert_clients),
                    NetworkTarget::from(update_clients),
                )
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
                if component_ticks.is_added(system_ticks.last_run(), system_ticks.this_run())
                    || replication_target.is_added()
                {
                    trace!("component is added or replication_target is added");
                    insert_target.union(target);
                } else {
                    // do not send updates for these components, only inserts/removes
                    if replicate_once {
                        trace!(?entity,
                                "not replicating updates for {:?} because it is marked as replicate_once",
                                "COMPONENT_KIND"
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
                    insert_target.union(&new_connected_target);
                }
                (insert_target, update_target)
            }
        };

        // do not send a component as both update and insert
        update_target.exclude(&insert_target);

        if !insert_target.is_empty() || !update_target.is_empty() {
            if !insert_target.is_empty() {
                let _ = sender
                    .prepare_component_insert(
                        entity,
                        component_kind,
                        component_data,
                        component_registry,
                        sync_target.map(|sync_target| &sync_target.prediction),
                        group_id,
                        insert_target,
                        delta_compression,
                        current_tick,
                        system_ticks.this_run(),
                    )
                    .inspect_err(|e| {
                        error!("error sending component insert: {:?}", e);
                    });
            }
            if !update_target.is_empty() {
                let _ = sender
                    .prepare_component_update(
                        entity,
                        component_kind,
                        component_data,
                        component_registry,
                        group_id,
                        update_target,
                        component_ticks.last_changed_tick(),
                        system_ticks.this_run(),
                        current_tick,
                        delta_compression,
                    )
                    .inspect_err(|e| {
                        error!("error sending component update: {:?}", e);
                    });
            }
        }
    }

    /// This system sends updates for all components that were removed
    pub(crate) fn send_component_removed<C: Component>(
        registry: Res<ComponentRegistry>,
        // only remove the component for entities that are being actively replicated
        query: Query<
            (
                &ReplicationTarget,
                &ReplicationGroup,
                Option<&CachedNetworkRelevance>,
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
                                    if matches!(visibility, ClientRelevance::Maintained) {
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

    pub(crate) fn register_replicate_component_send<C: Component>(app: &mut App) {
        app.add_systems(
            PostUpdate,
            (
                // NOTE: we need to run `send_component_removed` once per frame (and not once per send_interval)
                //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
                //  It is ok to run it every frame because it creates at most one message per despawn
                send_component_removed::<C>
                    .in_set(InternalReplicationSet::<ServerMarker>::BufferDespawnsAndRemovals),
                // // NOTE: we run this system once every `send_interval` because we don't want to send too many Update messages
                // //  and use up all the bandwidth
                // send_component_update::<C>
                //     .in_set(InternalReplicationSet::<ServerMarker>::BufferComponentUpdates),
            ),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::client::events::ComponentUpdateEvent;
        use crate::prelude::client::Confirmed;
        use crate::prelude::server::{ControlledBy, NetConfig, RelevanceManager, Replicate};
        use crate::prelude::{
            client, server, DeltaCompression, LinkConditionerConfig, ReplicateOnceComponent,
            Replicated,
        };
        use crate::server::replication::send::SyncTarget;
        use crate::shared::replication::components::{Controlled, ReplicationGroupId};
        use crate::shared::replication::delta::DeltaComponentHistory;
        use crate::shared::replication::systems;
        use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
        use crate::tests::protocol::*;
        use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
        use bevy::ecs::system::RunSystemOnce;
        use bevy::prelude::{default, EventReader, Resource, Update};
        use bevy::utils::HashSet;

        // TODO: test entity spawn newly connected client

        #[test]
        fn test_entity_spawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper.server_app.world_mut().spawn_empty().id();
            stepper.frame_step();
            stepper.frame_step();
            // check that entity wasn't spawned
            assert!(stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .is_none());

            // add replicate
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        interpolation: NetworkTarget::All,
                    },
                    controlled_by: ControlledBy {
                        target: NetworkTarget::All,
                        ..default()
                    },
                    ..default()
                });

            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that prediction, interpolation, controlled was handled correctly
            let confirmed = stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<Confirmed>()
                .expect("Confirmed component missing");
            assert!(confirmed.predicted.is_some());
            assert!(confirmed.interpolated.is_some());
            assert!(stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<Controlled>()
                .is_some());
        }

        #[test]
        fn test_multi_entity_spawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            stepper
                .server_app
                .world_mut()
                .spawn_batch(vec![Replicate::default(); 2]);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entities were spawned

            assert_eq!(
                stepper
                    .client_app
                    .world_mut()
                    .query_filtered::<(), With<Replicated>>()
                    .iter(stepper.client_app.world())
                    .len(),
                2
            );
        }

        #[test]
        fn test_entity_spawn_visibility() {
            let mut stepper = MultiBevyStepper::default();

            // spawn an entity on server with visibility::InterestManagement
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate {
                    relevance_mode: NetworkRelevanceMode::InterestManagement,
                    ..default()
                })
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that entity wasn't spawned
            assert!(stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .is_none());
            // make entity visible
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity);
            stepper.frame_step();
            stepper.frame_step();

            // check that entity was spawned
            let client_entity = *stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the entity was not spawned on the other client
            assert!(stepper
                .client_app_2
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .is_none());
        }

        #[test]
        fn test_entity_spawn_preexisting_target() {
            let mut stepper = BevyStepper::default();

            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn(ComponentSyncModeFull(1.0))
                .id();
            stepper.frame_step();
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    TargetEntity::Preexisting(client_entity),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was replicated on the client entity
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .resource::<client::ConnectionManager>()
                    .replication_receiver
                    .remote_entity_map
                    .get_local(server_entity)
                    .unwrap(),
                &client_entity
            );
            assert!(stepper
                .client_app
                .world()
                .get::<Replicated>(client_entity)
                .is_some());
            assert!(stepper
                .client_app
                .world()
                .get::<ComponentSyncModeFull>(client_entity)
                .is_some());
        }

        /// Check that if we change the replication target on an entity that already has one
        /// we spawn the entity for new clients
        #[test]
        fn test_entity_spawn_replication_target_update() {
            let mut stepper = MultiBevyStepper::default();

            // spawn an entity on server to client 1
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate {
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
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client 1");

            // update the replication target
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ReplicationTarget {
                    target: NetworkTarget::All,
                });
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity gets replicated to the other client
            stepper
                .client_app_2
                .world()
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
                .world_mut()
                .spawn(Replicate::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // despawn
            stepper.server_app.world_mut().despawn(server_entity);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned
            assert!(stepper
                .client_app
                .world()
                .get_entity(client_entity)
                .is_none());
        }

        /// Check that if interest management is used, a client losing visibility of an entity
        /// will cause the server to send a despawn-entity message to the client
        #[test]
        fn test_entity_despawn_lose_visibility() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate {
                    relevance_mode: NetworkRelevanceMode::InterestManagement,
                    ..default()
                })
                .id();
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID), server_entity);

            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // lose visibility
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .lose_relevance(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned
            assert!(stepper
                .client_app
                .world()
                .get_entity(client_entity)
                .is_none());
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
                .world_mut()
                .spawn(Replicate {
                    relevance_mode: NetworkRelevanceMode::InterestManagement,
                    group: ReplicationGroup::new_id(1),
                    ..default()
                })
                .id();
            let server_entity_2 = stepper
                .server_app
                .world_mut()
                .spawn(Replicate {
                    relevance_mode: NetworkRelevanceMode::InterestManagement,
                    group: ReplicationGroup::new_id(1),
                    ..default()
                })
                .id();
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity_1)
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID_2), server_entity_2);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned on each client
            let client_entity_1 = *stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity_1)
                .expect("entity was not replicated to client 1");
            let client_entity_2 = *stepper
                .client_app_2
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity_2)
                .expect("entity was not replicated to client 2");

            // update the entity_map on client 2 to re-use the same server entity as client 1
            // so that replication messages for server_entity_1 could also be read by client 2
            stepper
                .client_app_2
                .world_mut()
                .resource_mut::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .insert(server_entity_1, client_entity_2);

            // despawn the server_entity_1
            stepper.server_app.world_mut().despawn(server_entity_1);
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned on client 1
            assert!(stepper
                .client_app_1
                .world()
                .get_entity(client_entity_1)
                .is_none());

            // check that the entity still exists on client 2
            assert!(stepper
                .client_app_2
                .world()
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
                .world_mut()
                .spawn(Replicate {
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
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // update the replication target
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ReplicationTarget {
                    target: NetworkTarget::None,
                });
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned
            assert!(stepper
                .client_app
                .world()
                .get_entity(client_entity)
                .is_none());
        }

        #[test]
        fn test_component_insert() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(1.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
        }

        /// Use the non-delta replication for a component that has delta-compression functions registered
        #[test]
        fn test_component_insert_without_delta_for_delta_component() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentDeltaCompression(vec![3, 4]));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![3, 4])
            );
        }

        #[test]
        fn test_component_insert_delta() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::<ComponentDeltaCompression>::default(),
                ))
                .id();
            stepper.frame_step();
            let tick = stepper.server_tick();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2])
            );
            // check that the component value was stored in the delta manager cache
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    ReplicationGroupId(server_entity.to_bits())
                )
                .is_some());
        }

        #[test]
        fn test_component_insert_visibility_maintained() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate {
                    relevance_mode: NetworkRelevanceMode::InterestManagement,
                    ..default()
                })
                .id();
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(1.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
        }

        #[test]
        fn test_component_insert_visibility_gained() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate {
                    relevance_mode: NetworkRelevanceMode::InterestManagement,
                    ..default()
                })
                .id();

            stepper.frame_step();
            stepper.frame_step();

            // add component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(1.0));
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
            stepper.frame_step();
            stepper.frame_step();

            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
        }

        #[test]
        fn test_component_insert_disabled() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(Replicate::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert((
                    ComponentSyncModeFull(1.0),
                    DisabledComponent::<ComponentSyncModeFull>::default(),
                ));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was not replicated
            assert!(stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<ComponentSyncModeFull>()
                .is_none());
        }

        #[test]
        fn test_component_override_target() {
            let mut stepper = MultiBevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    ComponentSyncModeFull(1.0),
                    OverrideTargetComponent::<ComponentSyncModeFull>::new(NetworkTarget::Single(
                        ClientId::Netcode(TEST_CLIENT_ID_1),
                    )),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity_1 = *stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_entity_2 = *stepper
                .client_app_2
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // check that the component was replicated to client 1 only
            assert_eq!(
                stepper
                    .client_app_1
                    .world()
                    .entity(client_entity_1)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
            assert!(stepper
                .client_app_2
                .world()
                .entity(client_entity_2)
                .get::<ComponentSyncModeFull>()
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
                .world_mut()
                .spawn((
                    Replicate {
                        // target is both
                        relevance_mode: NetworkRelevanceMode::InterestManagement,
                        ..default()
                    },
                    ComponentSyncModeFull(1.0),
                    // override target is only client 1
                    OverrideTargetComponent::<ComponentSyncModeFull>::new(NetworkTarget::Single(
                        ClientId::Netcode(TEST_CLIENT_ID_1),
                    )),
                ))
                .id();
            // entity is visible to both
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity)
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID_2), server_entity);
            stepper.frame_step();
            stepper.frame_step();
            let client_entity_1 = *stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_entity_2 = *stepper
                .client_app_2
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // check that the component was replicated to client 1 only
            assert_eq!(
                stepper
                    .client_app_1
                    .world()
                    .entity(client_entity_1)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
            assert!(stepper
                .client_app_2
                .world()
                .entity(client_entity_2)
                .get::<ComponentSyncModeFull>()
                .is_none());
        }

        #[test]
        fn test_component_update() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((Replicate::default(), ComponentSyncModeFull(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // update component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(2.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(2.0)
            );
        }

        /// Test that replicating updates works even if the update happens after tick wrapping
        #[test]
        fn test_component_update_after_tick_wrap() {
            let mut stepper = BevyStepper::default();

            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((ComponentSyncModeFull(0.0), Replicate::default()))
                .id();

            // replicate to client
            stepper.frame_step();
            stepper.frame_step();

            // we increase the ticks in 2 steps (otherwise we would directly go over tick wrapping)
            let tick_delta = (u16::MAX / 3 + 10) as i16;
            stepper.set_client_tick(stepper.client_tick() + tick_delta);
            stepper.set_server_tick(stepper.server_tick() + tick_delta);

            stepper
                .server_app
                .world_mut()
                .run_system_once(systems::send_cleanup::<server::ConnectionManager>);
            stepper
                .client_app
                .world_mut()
                .run_system_once(systems::receive_cleanup::<client::ConnectionManager>);

            stepper.set_client_tick(stepper.client_tick() + tick_delta);
            stepper.set_server_tick(stepper.server_tick() + tick_delta);

            // update the component on the server
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(1.0));

            // make sure the client receives the replication message
            stepper.frame_step();
            stepper.frame_step();

            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .unwrap();
            // check that the component got updated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .get::<ComponentSyncModeFull>(client_entity)
                    .unwrap(),
                &ComponentSyncModeFull(1.0)
            );
        }

        #[test]
        fn test_component_update_send_frequency() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate {
                        // replicate every 4 ticks
                        group: ReplicationGroup::new_from_entity()
                            .set_send_frequency(Duration::from_millis(40)),
                        ..default()
                    },
                    ComponentSyncModeFull(1.0),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // update component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(2.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was not updated (because it had been only three ticks)
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
            // it has been 4 ticks, the component was updated
            stepper.frame_step();
            // check that the component was not updated (because it had been only two ticks)
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(2.0)
            );
        }

        #[test]
        fn test_component_update_delta() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::<ComponentDeltaCompression>::default(),
                ))
                .id();
            let group_id = ReplicationGroupId(server_entity.to_bits());
            stepper.frame_step();
            let insert_tick = stepper.server_tick();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2])
            );
            // check that the component value was stored in the delta manager cache
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    insert_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_some());

            // apply update
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression>()
                .unwrap()
                .0 = vec![1, 2, 3];
            stepper.frame_step();
            let update_tick = stepper.server_tick();
            // check that the delta manager has been updated correctly
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    insert_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_some());
            assert_eq!(
                *stepper
                    .server_app
                    .world()
                    .resource::<ConnectionManager>()
                    .delta_manager
                    .acks
                    .get(&group_id)
                    .expect("no acks data for the group_id found")
                    .get(&update_tick)
                    .unwrap(),
                1
            );
            stepper.frame_step();

            // check that the component was updated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2, 3])
            );
            // check that the component value for the update_tick was stored in the delta manager cache
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    update_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_some());
            // check that the component value for the insert_tick was removed from the delta manager cache
            // since all clients received the update for update_tick (so the insert_tick is no longer needed)
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    insert_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_none());
            // check that there is no acks data for the update tick, since all clients received the update
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .acks
                .get(&group_id)
                .expect("no acks data for the group_id found")
                .get(&update_tick)
                .is_none());
        }

        /// One component is delta, the other is not
        /// This fails to work if we don't have an ack tick specific to the delta component
        #[test]
        #[ignore]
        fn test_component_update_delta_with_non_delta_component() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    ComponentSyncModeFull(1.0),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::<ComponentDeltaCompression>::default(),
                ))
                .id();
            let group_id = ReplicationGroupId(server_entity.to_bits());
            stepper.frame_step();
            let insert_tick = stepper.server_tick();
            dbg!(insert_tick);
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2])
            );
            // check that the component value was stored in the delta manager cache
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    insert_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_some());

            // apply non-delta update
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentSyncModeFull>()
                .unwrap()
                .0 = 1.0;
            stepper.frame_step();
            stepper.frame_step();

            // apply update
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression>()
                .unwrap()
                .0 = vec![1, 2, 3];
            stepper.frame_step();
            let update_tick = stepper.server_tick();
            // check that the delta manager has been updated correctly
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    insert_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_some());
            assert_eq!(
                *stepper
                    .server_app
                    .world()
                    .resource::<ConnectionManager>()
                    .delta_manager
                    .acks
                    .get(&group_id)
                    .expect("no acks data for the group_id found")
                    .get(&update_tick)
                    .unwrap(),
                1
            );
            stepper.frame_step();

            // check that the component was updated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2, 3])
            );
            // check that the component value for the update_tick was stored in the delta manager cache
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    update_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_some());
            // check that the component value for the insert_tick was removed from the delta manager cache
            // since all clients received the update for update_tick (so the insert_tick is no longer needed)
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .data
                .get_component_value(
                    server_entity,
                    insert_tick,
                    ComponentKind::of::<ComponentDeltaCompression>(),
                    group_id,
                )
                .is_none());
            // check that there is no acks data for the update tick, since all clients received the update
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .acks
                .get(&group_id)
                .expect("no acks data for the group_id found")
                .get(&update_tick)
                .is_none());
        }

        /// We want to test the following case:
        /// - server sends a diff between ticks 1-3
        /// - client receives that and applies it
        /// - server sends a diff between ticks 1-5 (because the server hasn't received the
        ///   ack for tick 3 yet)
        /// - client receives that, applies it, and it still works even if client was already on tick 3
        ///
        /// We can emulate this by adding some delay on the server receiving client packets via the link conditioner.
        #[test]
        fn test_component_update_delta_non_idempotent_slow_ack() {
            // tracing_subscriber::FmtSubscriber::builder()
            //     .with_max_level(tracing::Level::DEBUG)
            //     .init();
            let mut stepper = BevyStepper::default();
            stepper.stop();

            #[allow(irrefutable_let_patterns)]
            if let NetConfig::Netcode { io, .. } = stepper
                .server_app
                .world_mut()
                .resource_mut::<ServerConfig>()
                .net
                .first_mut()
                .unwrap()
            {
                io.conditioner = Some(LinkConditionerConfig {
                    // the server receives client packets after 3 ticks
                    incoming_latency: Duration::from_millis(30),
                    incoming_jitter: Default::default(),
                    incoming_loss: 0.0,
                })
            }
            stepper.start();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::<ComponentDeltaCompression>::default(),
                ))
                .id();
            let group_id = ReplicationGroupId(server_entity.to_bits());
            stepper.frame_step();
            let insert_tick = stepper.server_tick();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2])
            );
            // apply update (we haven't received the ack from the client so our diff should be
            // from the base value, aka [2, 3])
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression>()
                .unwrap()
                .0 = vec![1, 2, 3];
            stepper.frame_step();
            let update_tick = stepper.server_tick();
            // check that the delta manager has been updated correctly
            // the update_tick message hasn't been acked yet
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .resource::<ConnectionManager>()
                    .delta_manager
                    .acks
                    .get(&group_id)
                    .expect("no acks data for the group_id found")
                    .get(&update_tick)
                    .expect("no acks count found for the update_tick"),
                &1
            );
            stepper.frame_step();
            // it still works because the DeltaType is FromBase, so the client re-applies the diff from the base value
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2, 3])
            );
            // the insert_tick message hasn't been acked yet
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .resource::<ConnectionManager>()
                    .delta_manager
                    .acks
                    .get(&group_id)
                    .expect("no acks data for the group_id found")
                    .get(&update_tick)
                    .expect("no acks count found for the update_tick"),
                &1
            );
            // wait a few ticks to be share that the server received the client ack
            stepper.frame_step();
            stepper.frame_step();
            stepper.frame_step();
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .acks
                .get(&group_id)
                .expect("no acks data for the group_id found")
                .get(&update_tick)
                .is_none());
            // apply update (the update should be from the last acked value, aka [4])
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression>()
                .unwrap()
                .0 = vec![1, 2, 3, 4];
            stepper.frame_step();
            let update_tick = stepper.server_tick();

            // apply another update (the update should still be from the last acked value, aka [4, 5])
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression>()
                .unwrap()
                .0 = vec![1, 2, 3, 4, 5];
            stepper.frame_step();
            // the client receives the first update
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2, 3, 4])
            );
            stepper.frame_step();
            // the client receives the second update, it still works well because we apply the diff
            // from the previous_value [1, 2, 3]
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression>()
                    .expect("component missing"),
                &ComponentDeltaCompression(vec![1, 2, 3, 4, 5])
            );
        }

        /// We want to test the following case:
        /// - server sends a diff between ticks 1-3
        /// - client receives that and applies it
        /// - server sends a diff between ticks 1-5 (because the server hasn't received the
        ///   ack for tick 3 yet)
        /// - client receives that, applies it, and it still works even if client was already on tick 3
        ///
        /// We can emulate this by adding some delay on the server receiving client packets via the link conditioner.
        #[test]
        fn test_component_update_delta_idempotent_slow_ack() {
            let mut stepper = BevyStepper::default();
            stepper.stop();

            #[allow(irrefutable_let_patterns)]
            if let NetConfig::Netcode { io, .. } = stepper
                .server_app
                .world_mut()
                .resource_mut::<ServerConfig>()
                .net
                .first_mut()
                .unwrap()
            {
                io.conditioner = Some(LinkConditionerConfig {
                    // the server receives client packets after 3 ticks
                    incoming_latency: Duration::from_millis(30),
                    incoming_jitter: Default::default(),
                    incoming_loss: 0.0,
                })
            }
            stepper.start();

            // spawn an entity on server (diff = added 1)
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    ComponentDeltaCompression2(HashSet::from([1])),
                    DeltaCompression::<ComponentDeltaCompression2>::default(),
                ))
                .id();
            let group_id = ReplicationGroupId(server_entity.to_bits());
            stepper.frame_step();
            let insert_tick = stepper.server_tick();
            // apply an update (diff = added 2 since we compute the diff FromBase)
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression2>()
                .unwrap()
                .0 = HashSet::from([2]);
            // replicate and make sure that the server received the client ack
            stepper.frame_step();
            let base_update_tick = stepper.server_tick();
            stepper.frame_step();
            stepper.frame_step();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression2>()
                    .expect("component missing"),
                &ComponentDeltaCompression2(HashSet::from([2]))
            );
            // check that the server received an ack
            assert!(stepper
                .server_app
                .world()
                .resource::<ConnectionManager>()
                .delta_manager
                .acks
                .get(&group_id)
                .expect("no acks data for the group_id found")
                .get(&base_update_tick)
                .is_none());
            // apply update (the update should be from the last acked value, aka add 3, remove 2)
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression2>()
                .unwrap()
                .0 = HashSet::from([3]);
            stepper.frame_step();
            let update_tick = stepper.server_tick();
            // apply another update (the update should still be from the last acked value, aka add 4, remove 2
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression2>()
                .unwrap()
                .0 = HashSet::from([4]);
            stepper.frame_step();
            // the client receives the first update
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression2>()
                    .expect("component missing"),
                &ComponentDeltaCompression2(HashSet::from([3]))
            );
            stepper.frame_step();
            // the client receives the second update, and it still works well because we apply the diff
            // against the stored history value
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression2>()
                    .expect("component missing"),
                &ComponentDeltaCompression2(HashSet::from([4]))
            );
            // check that the history still contains the component for the component update
            // (because we only purge when we receive a strictly more recent tick)
            assert!(stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<DeltaComponentHistory<ComponentDeltaCompression2>>()
                .expect("component missing")
                .buffer
                .contains_key(&update_tick));
            // but it doesn't contain the component for the initial insert
            assert!(!stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<DeltaComponentHistory<ComponentDeltaCompression2>>()
                .expect("component missing")
                .buffer
                .contains_key(&insert_tick));
        }

        /// Check that updates are not sent if the `ReplicationTarget` component gets removed.
        /// Check that updates are resumed when the `ReplicationTarget` component gets re-added.
        #[test]
        fn test_component_update_replication_target_removed() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((Replicate::default(), ComponentSyncModeFull(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // remove the replication_target component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(2.0))
                .remove::<ReplicationTarget>();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity still exists on the client, but that the component was not updated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );

            // re-add the replication_target component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ReplicationTarget::default());
            stepper.frame_step();
            stepper.frame_step();
            // check that the component gets updated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(2.0)
            );
        }

        #[test]
        fn test_component_update_disabled() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((Replicate::default(), ComponentSyncModeFull(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert((
                    ComponentSyncModeFull(2.0),
                    DisabledComponent::<ComponentSyncModeFull>::default(),
                ));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was not updated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
        }

        #[test]
        fn test_component_update_replicate_once() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    Replicate::default(),
                    ComponentSyncModeFull(1.0),
                    ReplicateOnceComponent::<ComponentSyncModeFull>::default(),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the component was replicated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );

            // update component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert(ComponentSyncModeFull(2.0));
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was not updated
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );
        }

        #[test]
        fn test_component_remove() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((Replicate::default(), ComponentSyncModeFull(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = *stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("component missing"),
                &ComponentSyncModeFull(1.0)
            );

            // remove component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .remove::<ComponentSyncModeFull>();
            stepper.frame_step();
            stepper.frame_step();

            // check that the component was replicated
            assert!(stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<ComponentSyncModeFull>()
                .is_none());
        }

        /// Check that if we switch the visibility mode, the entity gets spawned
        /// to the clients that now have visibility
        #[test]
        fn test_change_visibility_mode_spawn() {
            let mut stepper = BevyStepper::default();

            let server_entity = stepper
                .server_app
                .world_mut()
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
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert((
                    NetworkRelevanceMode::InterestManagement,
                    ReplicationTarget {
                        target: NetworkTarget::All,
                    },
                ));
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID), server_entity);

            stepper.frame_step();
            stepper.frame_step();
            stepper
                .client_app
                .world()
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
                |mut events: EventReader<ComponentUpdateEvent<ComponentSyncModeFull>>| {
                    if let Some(event) = events.read().next() {
                        panic!(
                            "ComponentUpdateEvent received for entity: {:?}",
                            event.entity()
                        );
                    }
                },
            );

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(ComponentSyncModeFull(1.0))
                .id();
            // spawn an entity on the client with the component value
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn(ComponentSyncModeFull(1.0))
                .id();

            // add replication with a pre-existing target
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert((
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
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(ComponentSyncModeFull(2.0))
                .id();
            // spawn an entity on the client with the component value
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn(ComponentSyncModeFull(1.0))
                .id();

            stepper.client_app.init_resource::<Counter>();
            stepper.client_app.add_systems(
                Update,
                move |mut events: EventReader<ComponentUpdateEvent<ComponentSyncModeFull>>,
                      mut counter: ResMut<Counter>| {
                    for events in events.read() {
                        counter.0 += 1;
                        assert_eq!(events.entity(), client_entity);
                    }
                },
            );

            // add replication with a pre-existing target
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .insert((
                    Replicate::default(),
                    TargetEntity::Preexisting(client_entity),
                ));

            // check that we did receive an ComponentUpdateEvent
            stepper.frame_step();
            stepper.frame_step();
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .get_resource::<Counter>()
                    .unwrap()
                    .0,
                1
            );
        }
    }
}

pub(crate) mod commands {
    use crate::channel::builder::AuthorityChannel;
    use crate::prelude::{Replicating, ServerConnectionManager};
    use crate::shared::replication::authority::{AuthorityChange, AuthorityPeer, HasAuthority};
    use bevy::ecs::system::EntityCommands;
    use bevy::prelude::{Entity, World};

    pub trait AuthorityCommandExt {
        /// This command is used to transfer the authority of an entity to a different peer.
        fn transfer_authority(&mut self, new_owner: AuthorityPeer);
    }

    impl AuthorityCommandExt for EntityCommands<'_> {
        fn transfer_authority(&mut self, new_owner: AuthorityPeer) {
            self.add(move |entity: Entity, world: &mut World| {
                // check who the current owner is
                let current_owner =
                    world
                        .get_entity(entity)
                        .map_or(AuthorityPeer::None, |entity| {
                            entity
                                .get::<AuthorityPeer>()
                                .copied()
                                .unwrap_or(AuthorityPeer::None)
                        });

                match (current_owner, new_owner) {
                    (x, y) if x == y => (),
                    (AuthorityPeer::None, AuthorityPeer::Server) => {
                        world
                            .entity_mut(entity)
                            .insert((HasAuthority, AuthorityPeer::Server));
                    }
                    (AuthorityPeer::None, AuthorityPeer::Client(c)) => {
                        world.entity_mut(entity).insert(AuthorityPeer::Client(c));
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &mut AuthorityChange {
                                    entity,
                                    gain_authority: true,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Server, AuthorityPeer::None) => {
                        world
                            .entity_mut(entity)
                            .remove::<HasAuthority>()
                            .insert(AuthorityPeer::None);
                    }
                    (AuthorityPeer::Client(c), AuthorityPeer::None) => {
                        world.entity_mut(entity).insert(AuthorityPeer::None);
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &mut AuthorityChange {
                                    entity,
                                    gain_authority: false,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Client(c), AuthorityPeer::Server) => {
                        // TODO: only gain the authority when we have received an ack
                        //  that the client has received the message?
                        world
                            .entity_mut(entity)
                            .insert((HasAuthority, AuthorityPeer::Server));
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &mut AuthorityChange {
                                    entity,
                                    gain_authority: false,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Server, AuthorityPeer::Client(c)) => {
                        world
                            .entity_mut(entity)
                            .remove::<HasAuthority>()
                            .insert(AuthorityPeer::Client(c));
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &mut AuthorityChange {
                                    entity,
                                    gain_authority: true,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Client(c1), AuthorityPeer::Client(c2)) => {
                        world.entity_mut(entity).insert(AuthorityPeer::Client(c2));
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c1,
                                &mut AuthorityChange {
                                    entity,
                                    gain_authority: false,
                                },
                            )
                            .expect("could not send message");
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c2,
                                &mut AuthorityChange {
                                    entity,
                                    gain_authority: true,
                                },
                            )
                            .expect("could not send message");
                    }
                    _ => unreachable!(),
                }
            });
        }
    }

    fn despawn_without_replication(entity: Entity, world: &mut World) {
        // remove replicating separately so that when we despawn the entity and trigger the observer
        // the entity doesn't have replicating anymore
        if let Some(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<Replicating>();
            entity_mut.despawn();
        }
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
        use bevy::prelude::With;

        use crate::prelude::server::Replicate;
        use crate::tests::protocol::*;
        use crate::tests::stepper::BevyStepper;

        use super::*;

        #[test]
        fn test_despawn() {
            let mut stepper = BevyStepper::default();

            let entity = stepper
                .server_app
                .world_mut()
                .spawn((ComponentSyncModeFull(1.0), Replicate::default()))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
                .client_app
                .world_mut()
                .query_filtered::<Entity, With<ComponentSyncModeFull>>()
                .get_single(stepper.client_app.world())
                .unwrap();

            // if we remove the Replicate bundle directly, and then despawn the entity
            // the despawn still gets replicated (because we removed ReplicationTarget while Replicating is still present)
            stepper
                .server_app
                .world_mut()
                .entity_mut(entity)
                .remove::<Replicate>();
            stepper.server_app.world_mut().entity_mut(entity).despawn();
            stepper.frame_step();
            stepper.frame_step();

            assert!(stepper
                .client_app
                .world_mut()
                .query::<&ComponentSyncModeFull>()
                .get_single(stepper.client_app.world())
                .is_err());

            // spawn a new entity
            let entity = stepper
                .server_app
                .world_mut()
                .spawn((ComponentSyncModeFull(1.0), Replicate::default()))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            assert!(stepper
                .client_app
                .world_mut()
                .query::<&ComponentSyncModeFull>()
                .get_single(stepper.client_app.world())
                .is_ok());

            // apply the command to remove replicate
            despawn_without_replication(entity, stepper.server_app.world_mut());
            stepper.frame_step();
            stepper.frame_step();
            // now the despawn should not have been replicated
            assert!(stepper
                .client_app
                .world_mut()
                .query::<&ComponentSyncModeFull>()
                .get_single(stepper.client_app.world())
                .is_ok());
        }
    }
}
