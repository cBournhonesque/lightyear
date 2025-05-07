use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use core::time::Duration;

use crate::client::components::Confirmed;
use crate::client::interpolation::Interpolated;
use crate::client::prediction::Predicted;
use crate::connection::client::NetClient;
use crate::prelude::client::ClientConnection;
use crate::prelude::{server::is_started, PrePredicted};
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
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
                        .after(InternalMainSet::<ServerMarker>::ReceiveEvents),
                );
        }
    }
}

pub(crate) mod send {
    use super::*;

    use crate::prelude::server::AuthorityCommandExt;
    use crate::prelude::{
        is_host_server, ChannelDirection, ClientId, ComponentRegistry, DeltaCompression,
        DisabledComponents, NetworkRelevanceMode, ReplicateLike, ReplicateOnce, ReplicationGroup,
        ShouldBePredicted, TargetEntity, Tick, TickManager, TimeManager,
    };
    use crate::protocol::component::ComponentKind;
    use crate::server::error::ServerError;
    use crate::server::prediction::handle_pre_predicted;
    use crate::server::relevance::immediate::{CachedNetworkRelevance, ClientRelevance};

    use crate::shared::replication::archetypes::{ReplicatedComponent, ServerReplicatedArchetypes};
    use crate::shared::replication::authority::{AuthorityPeer, HasAuthority};
    use crate::shared::replication::components::{
        Cached, Controlled, InitialReplicated, Replicating, ReplicationGroupId, ReplicationMarker,
        ShouldBeInterpolated,
    };
    use crate::shared::replication::network_target::NetworkTarget;
    use crate::shared::replication::ReplicationSend;
    use bevy::ecs::archetype::Archetypes;
    use bevy::ecs::component::{ComponentTicks, Components};

    use bevy::ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
    use bevy::ecs::world::FilteredEntityRef;
    use bevy::platform::collections::HashMap;
    use bevy::ptr::Ptr;

    use tracing::{debug, error, trace};

    #[derive(Default)]
    pub struct ServerReplicationSendPlugin {
        pub tick_interval: Duration,
    }

    impl Plugin for ServerReplicationSendPlugin {
        fn build(&self, app: &mut App) {
            let send_interval = app
                .world()
                .resource::<ServerConfig>()
                .shared
                .server_replication_send_interval;

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
                    InternalReplicationSet::<ServerMarker>::All.run_if(is_started),
                );
            // SYSTEMS
            app.add_systems(
                PostUpdate,
                (
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

            app.add_observer(replicate_entity_local_despawn);
            app.add_observer(add_has_authority_component);
            app.add_observer(handle_pre_predicted);
        }

        /// Wait until every component has been registered in the ComponentRegistry
        fn finish(&self, app: &mut App) {
            // temporarily remove component_registry from the app to enable split borrows
            let component_registry = app
                .world_mut()
                .remove_resource::<ComponentRegistry>()
                .unwrap();

            let replicate = (
                QueryParamBuilder::new(|builder| {
                    // Or<(With<ReplicateLike>, (With<Replicating>, With<ReplicateToClient>))>
                    builder.or(|b| {
                        b.with::<ReplicateLike>();
                        b.and(|b| {
                            b.with::<Replicating>();
                            b.with::<ReplicateToClient>();
                        });
                    });
                    builder.optional(|b| {
                        b.data::<(
                            &ReplicateLike,
                            &ReplicateToClient,
                            &ReplicationGroup,
                            &Cached<ReplicateToClient>,
                            &CachedNetworkRelevance,
                            &SyncTarget,
                            &TargetEntity,
                            &ControlledBy,
                            &AuthorityPeer,
                            &InitialReplicated,
                            &DisabledComponents,
                            &DeltaCompression,
                            &ReplicateOnce,
                            &OverrideTarget,
                        )>();
                        // include access to &C for all replication components with the right direction
                        component_registry
                            .replication_map
                            .iter()
                            .filter(|(_, m)| m.direction != ChannelDirection::ClientToServer)
                            .for_each(|(kind, _)| {
                                let id = component_registry.kind_to_component_id.get(kind).unwrap();
                                b.ref_id(*id);
                            });
                    });
                }),
                ParamBuilder,
                ParamBuilder,
                ParamBuilder,
                ParamBuilder,
                ParamBuilder,
                ParamBuilder,
                ParamBuilder,
            )
                .build_state(app.world_mut())
                .build_system(replicate);

            app.add_systems(
                PostUpdate,
                // TODO: putting it here means we might miss entities that are spawned and despawned within the send_interval? bug or feature?
                //  be careful that newly_connected_client is cleared every send_interval, not every frame.
                replicate
                    .in_set(InternalReplicationSet::<ServerMarker>::BufferEntityUpdates)
                    .in_set(InternalReplicationSet::<ServerMarker>::BufferComponentUpdates),
            );

            app.world_mut().insert_resource(component_registry);
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
    #[reflect(Component)]
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

    /// Component that indicates which clients the entity should be replicated to.
    #[derive(Component, Clone, Debug, PartialEq, Reflect)]
    #[reflect(Component)]
    #[require(ReplicationMarker, NetworkRelevanceMode)]
    pub struct ReplicateToClient {
        /// Which clients should this entity be replicated to
        pub target: NetworkTarget,
    }

    impl Default for ReplicateToClient {
        fn default() -> Self {
            Self {
                target: NetworkTarget::All,
            }
        }
    }

    // TODO: maybe have 3 fields:
    //  - target
    //  - override replication_target: bool (if true, we will completely override the replication target. If false, we do the intersection)
    //  - override visibility: bool (if true, we will completely override the visibility. If false, we do the intersection)
    /// This component lets you override the replication target for a specific component
    #[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
    #[reflect(Component)]
    pub struct OverrideTarget {
        overrides: HashMap<ComponentKind, NetworkTarget>,
    }

    impl OverrideTarget {
        /// Override the [`NetworkTarget`] for a given component
        pub fn insert<C: Component>(mut self, target: NetworkTarget) -> Self {
            self.overrides.insert(ComponentKind::of::<C>(), target);
            self
        }

        /// Clear the [`NetworkTarget`] override for the component
        pub fn clear<C: Component>(mut self, target: NetworkTarget) -> Self {
            self.overrides.remove(&ComponentKind::of::<C>());
            self
        }

        /// Get the overriding [`NetworkTarget`] for the component if there is one
        pub fn get<C: Component>(&self) -> Option<&NetworkTarget> {
            self.overrides.get(&ComponentKind::of::<C>())
        }

        /// Get the overriding [`NetworkTarget`] for the component if there is one
        pub(crate) fn get_kind(&self, component_kind: ComponentKind) -> Option<&NetworkTarget> {
            self.overrides.get(&component_kind)
        }
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
    /// - [`ReplicateToClient`] to specify which clients should receive the entity
    /// - [`SyncTarget`] to specify which clients should predict/interpolate the entity
    /// - [`ControlledBy`] to specify which client controls the entity
    /// - [`NetworkRelevanceMode`] to specify if we should replicate the entity to all clients in the
    ///   replication target, or if we should apply interest management logic to determine which clients
    /// - [`ReplicationGroup`] to group entities together for replication. Entities in the same group
    ///   will be sent together in the same message.
    /// - [`AuthorityPeer`] to change the peer that has authority (is allowed to send replication updates)
    ///   over an entity
    ///
    /// Some of the components can be updated at runtime even after the entity has been replicated.
    /// For example you can update the [`ReplicateToClient`] to change which clients should receive the entity.
    #[derive(Bundle, Clone, Default, PartialEq, Debug, Reflect)]
    pub struct Replicate {
        /// Which clients should this entity be replicated to?
        pub target: ReplicateToClient,
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
            Ref<ReplicateToClient>,
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
                        .insert((
                            Controlled,
                            DisabledComponents::default().disable::<Controlled>(),
                        ));
                }
            }
            if (replication_target.is_changed()) && replication_target.target.targets(&local_client)
            {
                // if pre_predicted.is_some_and(|pre_predicted| pre_predicted.client_entity.is_none())
                // {
                //     // PrePredicted's client_entity is None if it's a pre-predicted entity that was spawned by the local client
                //     // in that case, just remove it and add Predicted instead
                //     commands
                //         .entity(entity)
                //         .insert(Predicted {
                //             confirmed_entity: Some(entity),
                //         })
                //         .remove::<PrePredicted>();
                // }
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

    /// Keep a cached version of the [`ReplicateToClient`] component so that when it gets updated
    /// we can compute a diff with the previous value.
    ///
    /// This needs to run after we compute the diff, so after the `replicate` system runs
    pub(crate) fn handle_replication_target_update(
        mut commands: Commands,
        mut query: Query<
            (
                Entity,
                &ReplicateToClient,
                Option<&mut Cached<ReplicateToClient>>,
            ),
            Changed<ReplicateToClient>,
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
        q: Query<&AuthorityPeer>,
        mut commands: Commands,
    ) {
        let entity = trigger.target();
        if let Ok(authority_peer) = q.get(entity) {
            match authority_peer {
                AuthorityPeer::Client(c) => {
                    // immediately transfer authority to the client to make
                    // sure they know that they own it
                    commands.entity(entity).transfer_authority(*authority_peer);
                }
                AuthorityPeer::Server => {
                    trace!("Adding HasAuthority to {:?}", entity);
                    commands.entity(entity).insert(HasAuthority);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn replicate(
        // query for each &C + all replication components
        query: Query<FilteredEntityRef>,
        tick_manager: Res<TickManager>,
        component_registry: Res<ComponentRegistry>,
        system_ticks: SystemChangeTick,
        mut sender: ResMut<ConnectionManager>,
        archetypes: &Archetypes,
        components: &Components,
        mut replicated_archetypes: Local<ServerReplicatedArchetypes>,
    ) {
        replicated_archetypes.update(archetypes, components, component_registry.as_ref());

        // TODO: write and serialize in parallel (DashMap + pool of writers)
        query.iter().for_each(|entity_ref| {
            let entity = entity_ref.id();
            // If ReplicateLike is present, we will use the replication components from the parent entity
            // (unless the replication component is also present on the entity itself, in which case we overwrite the value)
            // TODO: where is disabled components used?
            let (
                group_id,
                priority,
                group_ready,
                cached_replication_target,
                visibility,
                sync_target,
                target_entity,
                controlled_by,
                authority_peer,
                initial_replicated,
                disabled_components,
                delta_compression,
                replicate_once,
                override_target,
                replication_target,
                is_replicate_like_added,
            ) = match entity_ref.get::<ReplicateLike>() { Some(replicate_like) => {
                // root entity does not exist
                let Ok(root_entity_ref) = query.get(replicate_like.0) else {
                    return;
                };
                if root_entity_ref.get::<ReplicateToClient>().is_none() {
                    // ReplicateLike points to a parent entity that doesn't have ReplicationToClient, skip
                    return;
                };
                let (group_id, priority, group_ready) =
                    entity_ref.get::<ReplicationGroup>().map_or_else(
                        // if ReplicationGroup is not present, we use the parent entity
                        || {
                            root_entity_ref
                                .get::<ReplicationGroup>()
                                .map(|g| {
                                    (
                                        g.group_id(Some(replicate_like.0)),
                                        g.priority(),
                                        g.should_send,
                                    )
                                })
                                .unwrap()
                        },
                        // we use the entity itself if ReplicationGroup is present
                        |g| (g.group_id(Some(entity)), g.priority(), g.should_send),
                    );
                (
                    group_id,
                    priority,
                    group_ready,
                    entity_ref
                        .get::<Cached<ReplicateToClient>>()
                        .or_else(|| root_entity_ref.get::<Cached<ReplicateToClient>>()),
                    entity_ref
                        .get::<CachedNetworkRelevance>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<SyncTarget>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<TargetEntity>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<ControlledBy>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<AuthorityPeer>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<InitialReplicated>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<DisabledComponents>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<DeltaCompression>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<ReplicateOnce>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get::<OverrideTarget>()
                        .or_else(|| root_entity_ref.get()),
                    entity_ref
                        .get_ref::<ReplicateToClient>()
                        .unwrap_or_else(|| root_entity_ref.get_ref::<ReplicateToClient>().unwrap()),
                    entity_ref.get_ref::<ReplicateLike>().unwrap().is_added(),
                )
            } _ => {
                if entity_ref.get::<ReplicateToClient>().is_none() {
                    // Skip entities with no ReplicateToClient
                    return;
                };
                let (group_id, priority, group_ready) = entity_ref
                    .get::<ReplicationGroup>()
                    .map(|g| (g.group_id(Some(entity)), g.priority(), g.should_send))
                    .unwrap();
                (
                    group_id,
                    priority,
                    group_ready,
                    entity_ref.get::<Cached<ReplicateToClient>>(),
                    entity_ref.get::<CachedNetworkRelevance>(),
                    entity_ref.get::<SyncTarget>(),
                    entity_ref.get::<TargetEntity>(),
                    entity_ref.get::<ControlledBy>(),
                    entity_ref.get::<AuthorityPeer>(),
                    entity_ref.get::<InitialReplicated>(),
                    entity_ref.get::<DisabledComponents>(),
                    entity_ref.get::<DeltaCompression>(),
                    entity_ref.get::<ReplicateOnce>(),
                    entity_ref.get::<OverrideTarget>(),
                    entity_ref.get_ref::<ReplicateToClient>().unwrap(),
                    false,
                )
            }};

            // add entity despawns from visibility or target change
            // (but not from entity despawn)
            replicate_entity_despawn(
                entity,
                group_id,
                &replication_target,
                cached_replication_target,
                authority_peer,
                visibility,
                &mut sender,
                &system_ticks
            );

            // add all entity spawns
            replicate_entity_spawn(
                &component_registry,
                entity,
                &replication_target,
                is_replicate_like_added,
                cached_replication_target,
                initial_replicated,
                group_id,
                priority,
                controlled_by,
                sync_target,
                target_entity,
                authority_peer,
                visibility,
                &mut sender,
                &system_ticks,
            );

            // If the group is not set to send, skip sending updates for this entity
            if !group_ready {
                return;
            }

            // NOTE: we pre-cache for each archetype the list of components that should be replicated
            // d. all components that were added or changed and that are not disabled
            for ReplicatedComponent { id, kind } in replicated_archetypes
                .archetypes
                .get(&entity_ref.archetype().id())
                .unwrap()
                .iter()
                .filter(|c| disabled_components.is_none_or(|d| d.enabled_kind(c.kind)))
            {
                let Some(data) = entity_ref.get_by_id(*id) else {
                    // component not present on entity, skip
                    return;
                };
                let component_ticks = entity_ref.get_change_ticks_by_id(*id).unwrap();

                let override_target = override_target.and_then(|o| o.get_kind(*kind));
                // TODO: maybe the old method was faster because we had-precached the delta-compression data
                //  for the archetype?
                let delta_compression = delta_compression.is_some_and(|d| d.enabled_kind(*kind));
                let replicate_once = replicate_once.is_some_and(|r| r.enabled_kind(*kind));

                replicate_component_updates(
                    tick_manager.tick(),
                    &component_registry,
                    entity,
                    *kind,
                    data,
                    component_ticks,
                    &replication_target,
                    sync_target,
                    group_id,
                    authority_peer,
                    visibility,
                    delta_compression,
                    replicate_once,
                    override_target,
                    &system_ticks,
                    &mut sender,
                );
            }
        })
    }

    /// Send entity spawn replication messages to clients
    /// Also handles:
    /// - an entity that becomes newly visible will be spawned remotely
    /// - send a spawn if the ReplicationTarget changes to include a new client
    /// - newly_connected_clients should receive the entity spawn message even if the entity was not just spawned
    /// - adding a ReplicateLike on an entity will send a Spawn for it
    /// - adds ControlledBy, ShouldBePredicted, ShouldBeInterpolated component
    /// - handles TargetEntity if it's a Preexisting entity
    pub(crate) fn replicate_entity_spawn(
        component_registry: &ComponentRegistry,
        entity: Entity,
        replication_target: &Ref<ReplicateToClient>,
        is_replicate_like_added: bool,
        cached_replication_target: Option<&Cached<ReplicateToClient>>,
        initial_replicated: Option<&InitialReplicated>,
        group_id: ReplicationGroupId,
        priority: f32,
        controlled_by: Option<&ControlledBy>,
        sync_target: Option<&SyncTarget>,
        target_entity: Option<&TargetEntity>,
        authority_peer: Option<&AuthorityPeer>,
        visibility: Option<&CachedNetworkRelevance>,
        connection_manager: &mut ConnectionManager,
        system_ticks: &SystemChangeTick,
    ) {
        // NOTE: we cannot use directly `is_changed` and `is_added` because of this bug
        // https://github.com/bevyengine/bevy/issues/13735
        let is_changed = replication_target.last_changed().is_newer_than(system_ticks.last_run(), system_ticks.this_run());
        let is_added = replication_target.added().is_newer_than(system_ticks.last_run(), system_ticks.this_run());

        let mut target = match visibility {
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
                                    if is_added {
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
                if is_replicate_like_added || is_added {
                    trace!(?entity, "send entity spawn");
                    // TODO: avoid this clone!
                    target = replication_target.target.clone();
                } else if is_changed {
                    target = replication_target.target.clone();
                    // if the replication target changed (for example from [1] to [1, 2]), do not replicate again to [1]
                    if let Some(cached_target) = cached_replication_target {
                        // do not re-send a spawn message to the clients for which we already have
                        // replicated the entity
                        target.exclude(&cached_target.value.target)
                    }
                }

                // also replicate to the newly connected clients that match the target
                let new_connected_clients = connection_manager.new_connected_clients();
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
        // we don't send messages to the client that has authority
        if let Some(AuthorityPeer::Client(c)) = authority_peer {
            target.exclude(&NetworkTarget::Single(*c));
        }

        // NOT NEEDED ANYMORE! WE DO SEND A SPAWN SO THAT THE CLIENT HAS A
        // - an action tick so that updates can now be received
        // - a GroupChannel with a Confirmed tick
        // // we don't send entity-spawn to the client who originally spawned the entity
        // if let Some(client_id) = initial_replicated.and_then(|r| r.from) {
        //     target.exclude(&NetworkTarget::Single(client_id));
        // };

        if target.is_empty() {
            return;
        }
        trace!(?entity, ?target, "Prepare entity spawn to client");
        let _ = crate::server::connection::connected_targets_mut(
            &mut connection_manager.connections,
            &target,
        )
        .try_for_each(|connection| {
            let client_id = connection.client_id;
            // convert the entity to a network entity (possibly mapped)
            // this can happen in the case of PrePrediction where the spawned entity has been pre-mapped
            // to the client's confirmed entity!
            let entity = connection
                .replication_receiver
                .remote_entity_map
                .to_remote(entity);

            // let the client know that this entity is controlled by them
            if controlled_by.is_some_and(|c| c.targets(&client_id)) {
                connection.prepare_typed_component_insert(
                    entity,
                    group_id,
                    component_registry,
                    &mut Controlled,
                )?;
            }
            // if we need to do prediction/interpolation, send a marker component to indicate that to the client
            if sync_target.is_some_and(|sync| sync.prediction.targets(&client_id)) {
                // TODO: the serialized data is always the same; cache it somehow?
                connection.prepare_typed_component_insert(
                    entity,
                    group_id,
                    component_registry,
                    &mut ShouldBePredicted,
                )?;
            }
            if sync_target.is_some_and(|sync| sync.interpolation.targets(&client_id)) {
                connection.prepare_typed_component_insert(
                    entity,
                    group_id,
                    component_registry,
                    &mut ShouldBeInterpolated,
                )?;
            }

            if let Some(TargetEntity::Preexisting(remote_entity)) = target_entity {
                connection.replication_sender.prepare_entity_spawn_reuse(
                    entity,
                    group_id,
                    *remote_entity,
                );
            } else {
                connection
                    .replication_sender
                    .prepare_entity_spawn(entity, group_id);
            }

            // also set the priority for the group when we spawn it
            connection
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
        trigger: Trigger<OnRemove, (ReplicationGroup, ReplicateLike)>,
        root_query: Query<&ReplicateLike>,
        // only replicate despawns to entities that still had Replicating at the time of their despawn
        query: Query<
            (
                &ReplicationGroup,
                &ReplicateToClient,
                Option<&CachedNetworkRelevance>,
            ),
            With<Replicating>,
        >,
        mut sender: ResMut<ConnectionManager>,
    ) {
        let entity = trigger.target();
        let root = root_query.get(entity).map_or(entity, |r| r.0);
        // TODO: be able to override the root components with the ones from the child if any are available!
        if let Ok((replication_group, network_target, cached_relevance)) = query.get(root) {
            // only send the despawn to clients who were in the target of the entity
            let mut target = network_target.clone().target;
            // only send the despawn to clients that had visibility of the entity
            if let Some(network_relevance) = cached_relevance {
                // TODO: optimize this in cases like All/None/Single/ExceptSingle
                target.intersection(&NetworkTarget::Only(
                    network_relevance.clients_cache.keys().copied().collect(),
                ))
            }
            let _ = sender
                .prepare_entity_despawn(entity, replication_group.group_id(Some(root)), target)
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
        replication_target: &Ref<ReplicateToClient>,
        cached_replication_target: Option<&Cached<ReplicateToClient>>,
        authority_peer: Option<&AuthorityPeer>,
        visibility: Option<&CachedNetworkRelevance>,
        sender: &mut ConnectionManager,
        system_ticks: &SystemChangeTick,
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

        // NOTE: we cannot use directly `is_changed` and `is_added` because of this bug
        // https://github.com/bevyengine/bevy/issues/13735
        let is_changed = replication_target.last_changed().is_newer_than(system_ticks.last_run(), system_ticks.this_run());
        let is_added = replication_target.added().is_newer_than(system_ticks.last_run(), system_ticks.this_run());

        // 2. if the replication target changed, find the clients that were removed in the new replication target
        if is_changed && !is_added {
            if let Some(cached_target) = cached_replication_target {
                // get targets that we had before but not anymore
                let mut new_despawn = cached_target.value.target.clone();
                new_despawn.exclude(&replication_target.target);
                target.union(&new_despawn);
            }
        }
        // 3. we don't send messages to the client that has authority
        if let Some(AuthorityPeer::Client(c)) = authority_peer {
            target.exclude(&NetworkTarget::Single(*c));
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
        replication_target: &Ref<ReplicateToClient>,
        sync_target: Option<&SyncTarget>,
        group_id: ReplicationGroupId,
        authority_peer: Option<&AuthorityPeer>,
        visibility: Option<&CachedNetworkRelevance>,
        delta_compression: bool,
        replicate_once: bool,
        override_target: Option<&NetworkTarget>,
        system_ticks: &SystemChangeTick,
        sender: &mut ConnectionManager,
    ) {
        // NOTE: we cannot use directly `is_changed` and `is_added` because of this bug
        // https://github.com/bevyengine/bevy/issues/13735
        let is_changed = replication_target.last_changed().is_newer_than(system_ticks.last_run(), system_ticks.this_run());
        let is_added = replication_target.added().is_newer_than(system_ticks.last_run(), system_ticks.this_run());

        // TODO: maybe iterate through all the connected clients instead, to avoid allocations?
        // use the overriden target if present
        let target = override_target.map_or(&replication_target.target, |override_target| {
            override_target
        });
        // if the replication target is added, we force an insert. This is to capture
        // existing components that were added before ReplicationTarget was added
        let force_insert = is_changed;
        // error!("force_insert: {}", force_insert);
        let (mut insert_target, mut update_target): (NetworkTarget, NetworkTarget) =
            match visibility {
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
                                        if component_ticks.is_added(
                                            system_ticks.last_run(),
                                            system_ticks.this_run(),
                                        ) || force_insert
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
                        || force_insert
                    {
                        trace!("component is added or replication_target is added");
                        insert_target.union(target);
                    } else {
                        // do not send updates for these components, only inserts/removes
                        if replicate_once {
                            trace!(?entity,
                                "not replicating updates for {:?} because it is marked as replicate_once",
                                component_kind
                            );
                        } else {
                            // otherwise send an update for all components that changed since the
                            // last update we have ack-ed
                            update_target.union(target);
                        }
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

        // we don't send messages to the client that has authority
        if let Some(AuthorityPeer::Client(c)) = authority_peer {
            insert_target.exclude(&NetworkTarget::Single(*c));
            update_target.exclude(&NetworkTarget::Single(*c));
        }

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
                        component_ticks.changed,
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
        trigger: Trigger<OnRemove, C>,
        registry: Res<ComponentRegistry>,
        child_query: Query<&ReplicateLike>,
        // only remove the component for entities that are being actively replicated
        query: Query<
            (
                &ReplicateToClient,
                &ReplicationGroup,
                Option<&AuthorityPeer>,
                Option<&CachedNetworkRelevance>,
                Option<&DisabledComponents>,
                Option<&OverrideTarget>,
            ),
            With<Replicating>,
        >,
        mut sender: ResMut<ConnectionManager>,
    ) {
        let entity = trigger.target();
        let kind = registry.net_id::<C>();
        // the root entity is either the ReplicateLike root or the entity itself
        let root_entity = child_query.get(entity).map_or(entity, |r| r.0);
        // TODO: allow overriding some components on the child
        if let Ok((
            replication_target,
            group,
            authority_peer,
            visibility,
            disabled_components,
            override_target,
        )) = query.get(root_entity)
        {
            // do not replicate components that are disabled
            if disabled_components.is_some_and(|d| !d.enabled::<C>()) {
                return;
            }
            // use the overriden target if present
            let base_target = override_target
                .and_then(|o| o.get::<C>())
                .unwrap_or(&replication_target.target);
            let mut target = match visibility {
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
            if let Some(AuthorityPeer::Client(c)) = authority_peer {
                target.exclude(&NetworkTarget::Single(*c));
            }
            if target.is_empty() {
                return;
            }
            let group_id = group.group_id(Some(entity));
            debug!(?entity, ?kind, "Sending RemoveComponent");
            let _ = sender.prepare_component_remove(entity, kind, group, target);
        }
    }

    pub(crate) fn register_replicate_component_send<C: Component>(app: &mut App) {
        app.add_observer(send_component_removed::<C>);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::client::events::ComponentUpdateEvent;
        use crate::prelude::client::Confirmed;
        use crate::prelude::server::{ControlledBy, NetConfig, RelevanceManager, Replicate};
        use crate::prelude::{client, server, ChannelDirection, DeltaCompression, LinkConditionerConfig, ReplicateOnce, Replicated, SharedConfig, TickConfig};
        use crate::server::replication::send::SyncTarget;
        use crate::shared::replication::components::{Controlled, ReplicationGroupId};
        use crate::shared::replication::delta::DeltaComponentHistory;
        use crate::shared::replication::systems;
        use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
        use crate::tests::protocol::*;
        use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
        use bevy::ecs::system::RunSystemOnce;
        use bevy::platform::collections::HashSet;
        use bevy::prelude::{default, EventReader, Resource, Update};
        use crate::client::config::ClientConfig;
        // TODO: test entity spawn newly connected client

        #[test]
        fn test_entity_spawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper.server_app.world_mut().spawn_empty().id();
            let server_child = stepper
                .server_app
                .world_mut()
                .spawn(ChildOf(server_entity))
                .id();
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
                .insert((
                    ReplicateToClient::default(),
                    SyncTarget {
                        prediction: NetworkTarget::All,
                        interpolation: NetworkTarget::All,
                    },
                    ControlledBy {
                        target: NetworkTarget::All,
                        ..default()
                    },
                ));

            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_child = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_child)
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

            // check that prediction, interpolation, controlled was handled correctly for the child
            let confirmed = stepper
                .client_app
                .world()
                .entity(client_child)
                .get::<Confirmed>()
                .expect("Confirmed component missing");
            assert!(confirmed.predicted.is_some());
            assert!(confirmed.interpolated.is_some());
            assert!(stepper
                .client_app
                .world()
                .entity(client_child)
                .get::<Controlled>()
                .is_some());
        }

        /// Test that if the replication send systems don't run every frame, we still correctly replicate entities
        #[test]
        fn test_entity_spawn_send_interval() {
            let tick_duration = Duration::from_millis(10);
            let mut stepper = BevyStepper::new(

                SharedConfig {
                    server_replication_send_interval: 2 * tick_duration,
                    client_replication_send_interval: 2 * tick_duration,
                    tick: TickConfig {
                        tick_duration,
                    },
                },
                ClientConfig::default(),
                tick_duration,
            );
            stepper.build();
            stepper.init();

            // spawn an entity on server
            let server_entity = stepper.server_app.world_mut().spawn(
                ReplicateToClient::default()
            ).id();
            stepper.advance_time(tick_duration);
            stepper.server_app.update();
            stepper.client_app.update();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
        }

        /// Check that a child is replicated correctly if ReplicateLike is added to it
        /// For example an existing entity is already replicated and we add a child to it.
        /// ReplicateLike should be added to the child, which should trigger a spawn.
        #[test]
        fn test_entity_spawn_child() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(ReplicateToClient::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // add a child to an already replicated entity
            let server_child = stepper
                .server_app
                .world_mut()
                .spawn(ChildOf(server_entity))
                .id();

            stepper.frame_step();
            stepper.frame_step();

            let client_child = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_child)
                .expect("entity was not replicated to client");
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
                .spawn((
                    ReplicateToClient::default(),
                    NetworkRelevanceMode::InterestManagement,
                ))
                .id();
            let server_child = stepper
                .server_app
                .world_mut()
                .spawn(ChildOf(server_entity))
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
            let client_entity = stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            // check that the child was also spawned because it copies the visibility of the parent
            let client_child = stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_child)
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
            assert!(stepper
                .client_app_2
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_child)
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
                    ReplicateToClient::default(),
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
                client_entity
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
                .spawn(ReplicateToClient {
                    target: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID_1)),
                })
                .id();
            stepper.frame_step();
            stepper.frame_step();

            let client_entity_1 = stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client 1");

            // update the replication target
            // we purposefully use a mutation instead of an update so that no observers are triggered
            // TODO: switch to immutable component + OnReplace observer
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ReplicateToClient>()
                .unwrap()
                .target = NetworkTarget::All;
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
                .spawn(ReplicateToClient::default())
                .id();
            let server_child = stepper
                .server_app
                .world_mut()
                .spawn(ChildOf(server_entity))
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_child = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_child)
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
                .is_err());
            // check that the child was despawned
            assert!(stepper.client_app.world().get_entity(client_child).is_err());
        }

        /// Check that a despawn of a child with ReplicateLike is replicated
        #[test]
        fn test_entity_despawn_child() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(ReplicateToClient::default())
                .id();

            let server_child = stepper
                .server_app
                .world_mut()
                .spawn(ChildOf(server_entity))
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_child = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_child)
                .expect("entity was not replicated to client");

            // despawn
            stepper.server_app.world_mut().despawn(server_child);
            stepper.frame_step();
            stepper.frame_step();

            // check that the child was despawned
            assert!(stepper.client_app.world().get_entity(client_child).is_err());
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
                .spawn((
                    ReplicateToClient::default(),
                    NetworkRelevanceMode::InterestManagement,
                ))
                .id();
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID), server_entity);

            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was spawned
            let client_entity = stepper
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
                .is_err());
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
                .spawn((
                    ReplicateToClient::default(),
                    NetworkRelevanceMode::InterestManagement,
                    ReplicationGroup::new_id(1),
                ))
                .id();
            let server_entity_2 = stepper
                .server_app
                .world_mut()
                .spawn((
                    ReplicateToClient::default(),
                    NetworkRelevanceMode::InterestManagement,
                    ReplicationGroup::new_id(1),
                ))
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
            let client_entity_1 = stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity_1)
                .expect("entity was not replicated to client 1");
            let client_entity_2 = stepper
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
                .is_err());

            // check that the entity still exists on client 2
            assert!(stepper
                .client_app_2
                .world()
                .get_entity(client_entity_2)
                .is_ok());
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
                .spawn(ReplicateToClient {
                    target: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID)),
                })
                .id();
            stepper.frame_step();
            stepper.frame_step();

            let client_entity = stepper
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
                .get_mut::<ReplicateToClient>()
                .unwrap()
                .target = NetworkTarget::None;
            stepper.frame_step();
            stepper.frame_step();

            // check that the entity was despawned
            assert!(stepper
                .client_app
                .world()
                .get_entity(client_entity)
                .is_err());
        }

        #[test]
        fn test_component_insert() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn(ReplicateToClient::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                .spawn(ReplicateToClient::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
            // tracing_subscriber::FmtSubscriber::builder()
            //     .with_max_level(tracing::Level::DEBUG)
            //     .init();
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    ReplicateToClient::default(),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::default().add::<ComponentDeltaCompression>(),
                ))
                .id();
            stepper.frame_step();
            let tick = stepper.server_tick();
            stepper.frame_step();
            let client_entity = stepper
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
                .spawn((
                    ReplicateToClient::default(),
                    NetworkRelevanceMode::InterestManagement,
                ))
                .id();
            stepper
                .server_app
                .world_mut()
                .resource_mut::<RelevanceManager>()
                .gain_relevance(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                .spawn((
                    ReplicateToClient::default(),
                    NetworkRelevanceMode::InterestManagement,
                ))
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

            let client_entity = stepper
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
                .spawn(ReplicateToClient::default())
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                    DisabledComponents::default().disable::<ComponentSyncModeFull>(),
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
                    ReplicateToClient::default(),
                    ComponentSyncModeFull(1.0),
                    OverrideTarget::default().insert::<ComponentSyncModeFull>(
                        NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID_1)),
                    ),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity_1 = stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_entity_2 = stepper
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
                    // target is both
                    ReplicateToClient::default(),
                    NetworkRelevanceMode::InterestManagement,
                    ComponentSyncModeFull(1.0),
                    // override target is only client 1
                    OverrideTarget::default().insert::<ComponentSyncModeFull>(
                        NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID_1)),
                    ),
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
            let client_entity_1 = stepper
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_entity_2 = stepper
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
                .spawn((ReplicateToClient::default(), ComponentSyncModeFull(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                .spawn((ComponentSyncModeFull(0.0), ReplicateToClient::default()))
                .id();

            // replicate to client
            stepper.frame_step();
            stepper.frame_step();

            // we increase the ticks in 2 steps (otherwise we would directly go over tick wrapping)
            let tick_delta = (u16::MAX / 3 + 10) as i16;
            stepper.set_client_tick(stepper.client_tick() + tick_delta);
            stepper.set_server_tick(stepper.server_tick() + tick_delta);

            let _ = stepper
                .server_app
                .world_mut()
                .run_system_once(systems::send_cleanup::<server::ConnectionManager>);
            let _ = stepper
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

            let client_entity = stepper
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
                    ReplicateToClient::default(),
                    // replicate every 4 ticks
                    ReplicationGroup::new_from_entity()
                        .set_send_frequency(Duration::from_millis(40)),
                    ComponentSyncModeFull(1.0),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                    ReplicateToClient::default(),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::default().add::<ComponentDeltaCompression>(),
                ))
                .id();
            let group_id = ReplicationGroupId(server_entity.to_bits());
            stepper.frame_step();
            let insert_tick = stepper.server_tick();
            stepper.frame_step();
            let client_entity = stepper
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
        fn test_component_update_delta_with_non_delta_component() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    ReplicateToClient::default(),
                    ComponentSyncModeFull(1.0),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::default().add::<ComponentDeltaCompression>(),
                ))
                .id();
            let group_id = ReplicationGroupId(server_entity.to_bits());
            stepper.frame_step();
            let insert_tick = stepper.server_tick();
            stepper.frame_step();
            let client_entity = stepper
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
                    ReplicateToClient::default(),
                    ComponentDeltaCompression(vec![1, 2]),
                    DeltaCompression::default().add::<ComponentDeltaCompression>(),
                ))
                .id();
            let group_id = ReplicationGroupId(server_entity.to_bits());
            stepper.frame_step();
            let insert_tick = stepper.server_tick();
            stepper.frame_step();
            let client_entity = stepper
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
                    ReplicateToClient::default(),
                    ComponentDeltaCompression2(HashSet::from_iter([1])),
                    DeltaCompression::default().add::<ComponentDeltaCompression2>(),
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
                .0 = HashSet::from_iter([2]);
            // replicate and make sure that the server received the client ack
            stepper.frame_step();
            let base_update_tick = stepper.server_tick();
            stepper.frame_step();
            stepper.frame_step();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                &ComponentDeltaCompression2(HashSet::from_iter([2]))
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
                .0 = HashSet::from_iter([3]);
            stepper.frame_step();
            let update_tick = stepper.server_tick();
            // apply another update (the update should still be from the last acked value, aka add 4, remove 2
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .get_mut::<ComponentDeltaCompression2>()
                .unwrap()
                .0 = HashSet::from_iter([4]);
            stepper.frame_step();
            // the client receives the first update
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_entity)
                    .get::<ComponentDeltaCompression2>()
                    .expect("component missing"),
                &ComponentDeltaCompression2(HashSet::from_iter([3]))
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
                &ComponentDeltaCompression2(HashSet::from_iter([4]))
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
                .spawn((ReplicateToClient::default(), ComponentSyncModeFull(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                .remove::<ReplicateToClient>();
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
                .insert(ReplicateToClient::default());
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
                .spawn((ReplicateToClient::default(), ComponentSyncModeFull(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
                    DisabledComponents::default().disable::<ComponentSyncModeFull>(),
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
                    ReplicateToClient::default(),
                    ComponentSyncModeFull(1.0),
                    ReplicateOnce::default().add::<ComponentSyncModeFull>(),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
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
        fn test_component_update_replicate_once_new_client() {
            let mut stepper = BevyStepper::default_no_init();

            // spawn an entity on server (before the client is connected)
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((
                    ReplicateToClient::default(),
                    ComponentSyncModeFull(1.0),
                    ReplicateOnce::default().add::<ComponentSyncModeFull>(),
                ))
                .id();
            stepper.frame_step();

            // a client connects
            stepper.init();
            stepper.frame_step();
            stepper.frame_step();

            let client_entity = stepper
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
                .spawn((ReplicateToClient::default(), ComponentSyncModeFull(1.0)))
                .id();
            let server_child = stepper
                .server_app
                .world_mut()
                .spawn((
                    ChildOf(server_entity),
                    ComponentSyncModeOnce(1.0),
                ))
                .id();
            stepper.frame_step();
            stepper.frame_step();
            let client_entity = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");
            let client_child = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_child)
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
            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .entity(client_child)
                    .get::<ComponentSyncModeOnce>()
                    .expect("component missing"),
                &ComponentSyncModeOnce(1.0)
            );

            // remove component
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_entity)
                .remove::<ComponentSyncModeFull>();
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_child)
                .remove::<ComponentSyncModeOnce>();
            stepper.frame_step();
            stepper.frame_step();

            // check that the remove was replicated
            assert!(stepper
                .client_app
                .world()
                .entity(client_entity)
                .get::<ComponentSyncModeFull>()
                .is_none());
            assert!(stepper
                .client_app
                .world()
                .entity(client_child)
                .get::<ComponentSyncModeOnce>()
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
                .spawn(ReplicateToClient {
                    target: NetworkTarget::None,
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
                    ReplicateToClient {
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

        /// Make sure that ClientToServer components are not replicated to the client
        #[test]
        fn test_component_direction() {
            let mut stepper = BevyStepper::default();

            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .resource::<ComponentRegistry>()
                    .direction(ComponentKind::of::<ComponentClientToServer>()),
                Some(ChannelDirection::ClientToServer)
            );

            // spawn an entity on server
            let server_entity = stepper
                .server_app
                .world_mut()
                .spawn((Replicate::default(), ComponentClientToServer(1.0)))
                .id();
            stepper.frame_step();
            stepper.frame_step();

            let client_entity = stepper
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .expect("entity was not replicated to client");

            // check that the component was not replicated to the server
            assert!(stepper
                .client_app
                .world()
                .get::<ComponentClientToServer>(client_entity)
                .is_none());
        }
    }
}

pub(crate) mod commands {
    use crate::channel::builder::AuthorityChannel;
    use crate::prelude::server::{ReplicateToClient, SyncTarget};
    use crate::prelude::{
        ClientId, PrePredicted, Replicated, Replicating, ReplicationGroup, ServerConnectionManager,
    };
    use crate::shared::replication::authority::{AuthorityChange, AuthorityPeer, HasAuthority};
    use crate::shared::replication::components::{InitialReplicated, ReplicationGroupId};
    use bevy::ecs::system::EntityCommands;
    use bevy::prelude::{EntityWorldMut, World};

    pub trait AuthorityCommandExt {
        /// This command is used to transfer the authority of an entity to a different peer.
        fn transfer_authority(&mut self, new_owner: AuthorityPeer);
    }

    impl AuthorityCommandExt for EntityCommands<'_> {
        fn transfer_authority(&mut self, new_owner: AuthorityPeer) {
            self.queue(move |entity_mut: EntityWorldMut| {
                let entity = entity_mut.id();
                let world = entity_mut.into_world_mut();
                let bevy_tick = world.change_tick();
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

                let compute_sync_target = |world: &World, c: ClientId| {
                    if world.get::<PrePredicted>(entity).is_some() {
                        return (false, false)
                    }
                    let initial_replicated = world.get::<InitialReplicated>(entity);
                    let sync_target = world.get::<SyncTarget>(entity);
                    // if the entity was originally spawned by a client C1,
                    // then C1 might want to add prediction or interpolation now that they lose authority
                    // over it
                    let add_prediction = initial_replicated.is_some_and(|initial| {
                        initial.from == Some(c)
                            && sync_target.is_some_and(|target| target.prediction.targets(&c))
                    });
                    let add_interpolation = initial_replicated.is_some_and(|initial| {
                        initial.from == Some(c)
                            && sync_target.is_some_and(|target| target.interpolation.targets(&c))
                    });
                    (add_prediction, add_interpolation)
                };

                // send a Spawn message (so that the receiver has a receiver GroupChannel with a Confirmed tick)
                // and make sure that the server doesn't send replication updates to the previous authoritative client
                // by updating the send_tick to the current tick, so that only changes after this tick are sent
                let spawn_and_update_send_tick = |world: &mut World, c: ClientId| {
                    // for pre-prediction, we don't need to do anything
                    // - the ReplicationTarget is added *after* the authority is changed, so a Spawn message is sent
                    // - prediction is already handled
                    if world.get::<PrePredicted>(entity).is_some() {
                        return
                    }
                    // check that the entity has the Replicate bundle
                    // - so that the authority components are correct
                    // - so that we know the send-group-id of the entity
                    assert!(world.get::<ReplicateToClient>(entity).is_some(), "The Replicate bundle must be added to the entity BEFORE transferring authority to the server");
                    let group_id = world.get::<ReplicationGroup>(entity).map_or(ReplicationGroupId(entity.to_bits()), |group| group.group_id(Some(entity)));
                    // if the entity was initially replicated from this client, then we need to spawn it back
                    // to that client:
                    // - so that the client's has a receiver GroupChannel with a confirmed tick
                    // - to add prediction/interpolation if necessary
                    if world.get::<InitialReplicated>(entity).is_some_and(|r| r.from == Some(c)) {
                        let network_entity =  world
                            .resource_mut::<ServerConnectionManager>()
                            .connection_mut(c)
                            .expect("could not get connection when changing authority")
                            .replication_receiver
                            .remote_entity_map
                            .to_remote(entity);
                        // NOTE: we cannot send ShouldBePredicted/ShouldBeInterpolated here because there is a chance
                        //  that the EntityAction message arrives before the AuthorityTransfer message arrives.
                        //  In which case the ComponentInserts/Actions (ShouldBePredicted) will be ignored since the
                        //  client 1 still has authority!
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .connection_mut(c)
                            .expect("could not get connection when changing authority").replication_sender.prepare_entity_spawn(network_entity, group_id);
                    }
                    world
                        .resource_mut::<ServerConnectionManager>()
                        .connection_mut(c)
                        .expect("could not get connection when changing authority")
                        .replication_sender
                        .group_channels
                        .entry(group_id)
                        .or_default()
                        .send_tick = Some(bevy_tick);
                };

                // TODO: handle authority transfers in host-server mode!
                //  when transferring to local-client, we want to transfer to the server instead?
                match (current_owner, new_owner) {
                    (x, y) if x == y => (),
                    (AuthorityPeer::None, AuthorityPeer::Server) => {
                        world
                            .entity_mut(entity)
                            .insert((HasAuthority, AuthorityPeer::Server));
                    }
                    (AuthorityPeer::None, AuthorityPeer::Client(c)) => {
                        world
                            .entity_mut(entity)
                            .insert((AuthorityPeer::Client(c), Replicated { from: Some(c) }));
                        let (add_prediction, add_interpolation) = compute_sync_target(world, c);
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &AuthorityChange {
                                    entity,
                                    gain_authority: true,
                                    add_prediction,
                                    add_interpolation,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Server, AuthorityPeer::None) => {
                        world
                            .entity_mut(entity)
                            .remove::<(HasAuthority, Replicated)>()
                            .insert(AuthorityPeer::None);
                    }
                    (AuthorityPeer::Client(c), AuthorityPeer::None) => {
                        world
                            .entity_mut(entity)
                            .remove::<Replicated>()
                            .insert(AuthorityPeer::None);
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &AuthorityChange {
                                    entity,
                                    gain_authority: false,
                                    add_prediction: false,
                                    add_interpolation: false,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Client(c), AuthorityPeer::Server) => {
                        let (add_prediction, add_interpolation) = compute_sync_target(world, c);
                        world
                            .entity_mut(entity)
                            .remove::<Replicated>()
                            .insert((HasAuthority, AuthorityPeer::Server));

                        spawn_and_update_send_tick(world, c);

                        // now that it has authority, by updating the
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &AuthorityChange {
                                    entity,
                                    gain_authority: false,
                                    add_prediction,
                                    add_interpolation,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Server, AuthorityPeer::Client(c)) => {
                        world
                            .entity_mut(entity)
                            .remove::<HasAuthority>()
                            .insert((AuthorityPeer::Client(c), Replicated { from: Some(c) }));
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c,
                                &AuthorityChange {
                                    entity,
                                    gain_authority: true,
                                    // TODO: should we compute these again?
                                    add_prediction: false,
                                    add_interpolation: false,
                                },
                            )
                            .expect("could not send message");
                    }
                    (AuthorityPeer::Client(c1), AuthorityPeer::Client(c2)) => {
                        world
                            .entity_mut(entity)
                            .insert((AuthorityPeer::Client(c2), Replicated { from: Some(c2) }));
                        let (add_prediction, add_interpolation) = compute_sync_target(world, c1);
                        spawn_and_update_send_tick(world, c1);
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c1,
                                & AuthorityChange {
                                    entity,
                                    gain_authority: false,
                                    add_prediction,
                                    add_interpolation,
                                },
                            )
                            .expect("could not send message");
                        world
                            .resource_mut::<ServerConnectionManager>()
                            .send_message::<AuthorityChannel, _>(
                                c2,
                                &AuthorityChange {
                                    entity,
                                    gain_authority: true,
                                    add_prediction: false,
                                    add_interpolation: false,
                                },
                            )
                            .expect("could not send message");
                    }
                    _ => unreachable!(),
                }
            });
        }
    }

    fn despawn_without_replication(mut entity_mut: EntityWorldMut) {
        // remove replicating separately so that when we despawn the entity and trigger the observer
        // the entity doesn't have replicating anymore
        entity_mut.remove::<Replicating>();
        entity_mut.despawn();
    }

    pub trait DespawnReplicationCommandExt {
        /// Despawn the entity and makes sure that the despawn won't be replicated.
        fn despawn_without_replication(&mut self);
    }
    impl DespawnReplicationCommandExt for EntityCommands<'_> {
        fn despawn_without_replication(&mut self) {
            self.queue(despawn_without_replication);
        }
    }

    #[cfg(test)]
    mod tests {
        use bevy::prelude::{Entity, With};

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
                .single(stepper.client_app.world())
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
                .single(stepper.client_app.world())
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
                .single(stepper.client_app.world())
                .is_ok());

            // apply the command to remove replicate
            despawn_without_replication(stepper.server_app.world_mut().entity_mut(entity));
            stepper.frame_step();
            stepper.frame_step();
            // now the despawn should not have been replicated
            assert!(stepper
                .client_app
                .world_mut()
                .query::<&ComponentSyncModeFull>()
                .single(stepper.client_app.world())
                .is_ok());
        }
    }
}
