//! Client replication plugins
use bevy::prelude::*;
use core::time::Duration;

use crate::client::connection::ConnectionManager;
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

pub(crate) mod receive {
    use super::*;
    use crate::client::message::ReceiveMessage;
    use crate::prelude::{
        client::{is_connected, is_synced},
        is_host_server, ClientConnectionManager, Replicated, ReplicationGroup, ShouldBePredicted,
    };
    use crate::shared::replication::authority::{AuthorityChange, HasAuthority};
    use crate::shared::replication::components::{ReplicationGroupId, ShouldBeInterpolated};
    use crate::shared::sets::InternalMainSet;
    use bevy::ecs::entity::Entities;
    use tracing::{debug, trace};

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

            app.configure_sets(
                PostUpdate,
                // only replicate entities once client is synced
                // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
                //  and the message might be ignored by the server
                //  But then pre-predicted entities that are spawned right away will not be replicated?
                // NOTE: we always need to add this condition if we don't enable replication, because
                InternalReplicationSet::<ClientMarker>::All
                    .run_if(is_connected.and(is_synced).and(not(is_host_server))),
            );

            app.add_systems(
                PreUpdate,
                handle_authority_change.after(InternalMainSet::<ClientMarker>::ReceiveEvents),
            );
        }
    }

    /// Apply authority changes requested by the server
    /// - remove/add HasAuthority
    /// - remove/add Replicated
    ///
    /// - add the entity to the ReplicationReceiver if we lose authority and we were the original spawner of the entity.
    ///
    /// The reason is that upon losing authority we might want to add Interpolation/Prediction to the entity.
    /// (client C1 spawned entity and authority passes to server).
    ///
    /// We want the entity in the ReplicationReceiver (especially local_entity_to_group) so that the
    /// Confirmed tick of the entity can keep being updated.
    // TODO: use observer to handle these?
    fn handle_authority_change(
        mut commands: Commands,
        entities: &Entities,
        mut messages: ResMut<Events<ReceiveMessage<AuthorityChange>>>,
    ) {
        for message_event in messages.drain() {
            let message = message_event.message;
            let entity = message.entity;
            debug!("Received authority change for entity {entity:?}");
            if entities.get(entity).is_some() {
                if message.gain_authority {
                    commands.queue(move |world: &mut World| {
                        let bevy_tick = world.change_tick();
                        // check that the entity has ReplicationGroup bundle
                        assert!(world.get::<ReplicationGroup>(entity).is_some(), "The Replicate bundle must be added to the entity BEFORE transferring authority to the client");
                        let group_id = world.get::<ReplicationGroup>(entity).map_or(ReplicationGroupId(entity.to_bits()), |group| group.group_id(Some(entity)));

                        world.entity_mut(entity).remove::<Replicated>().insert(HasAuthority);

                        trace!("Gain authority for entity {:?} with group {:?}", entity, group_id);
                        // TODO: do we need to send a Spawn so that the receiver has a ReplicationGroup
                        //  with the correct local_entities?
                        //  - if the last_action_tick is None (in case of authority transfer), then the receiver will
                        //    accept Updates
                        //  - the server doesn't use Confirmed tick
                        //  so maybe not?
                        // let network_entity =  world
                        //     .resource_mut::<ClientConnectionManager>()
                        //     .replication_receiver
                        //     .remote_entity_map
                        //     .to_remote(entity);
                        // world
                        //     .resource_mut::<ClientConnectionManager>()
                        //     .replication_sender
                        //     .prepare_entity_spawn(network_entity, group_id);

                        // make sure that the client doesn't start sending redundant replication updates
                        // only send updates that happened after the client received the authority change
                        world
                            .resource_mut::<ClientConnectionManager>()
                            .replication_sender
                            .group_channels
                            .entry(group_id)
                            .or_default()
                            .send_tick = Some(bevy_tick);


                    });
                } else {
                    // TODO: how do we know if the remote is still actively replicating to us?
                    //  for example is the new authority is None, then we don't want to add Replicated, no?
                    //  Not sure how to handle this. We could include in the message if the authority is None,
                    //  but that's not very elegant
                    commands.queue(move |world: &mut World| {
                        world
                            .entity_mut(entity)
                            .remove::<HasAuthority>()
                            .insert(Replicated { from: None });
                        if message.add_prediction {
                            world.entity_mut(entity).insert(ShouldBePredicted);
                        }
                        if message.add_interpolation {
                            world.entity_mut(entity).insert(ShouldBeInterpolated);
                        }
                    })
                }
            }
        }
    }
}

pub(crate) mod send {
    use super::*;
    use bevy::ecs::archetype::Archetypes;
    use bevy::ecs::component::{ComponentTicks, Components};

    use crate::connection::client::ClientConnection;

    use crate::prelude::client::{ClientConfig, NetClient};

    use crate::prelude::{
        client::{is_connected, is_synced},
        is_host_server, ChannelDirection, ComponentRegistry, DeltaCompression, DisabledComponents,
        ReplicateLike, ReplicateOnce, Replicated, ReplicationGroup, TargetEntity, Tick,
        TickManager, TimeManager,
    };
    use crate::protocol::component::ComponentKind;

    use crate::shared::replication::components::{
        InitialReplicated, Replicating, ReplicationGroupId,
    };

    use crate::shared::replication::archetypes::{ClientReplicatedArchetypes, ReplicatedComponent};
    use crate::shared::replication::authority::HasAuthority;
    use crate::shared::replication::components::ReplicationMarker;
    use crate::shared::replication::error::ReplicationError;

    use bevy::ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
    use bevy::ecs::world::FilteredEntityRef;
    use bevy::ptr::Ptr;

    use tracing::{debug, error, trace};

    #[derive(Default)]
    pub struct ClientReplicationSendPlugin {
        pub tick_interval: Duration,
    }

    impl Plugin for ClientReplicationSendPlugin {
        fn build(&self, app: &mut App) {
            let send_interval = app
                .world()
                .resource::<ClientConfig>()
                .shared
                .client_replication_send_interval;

            app
                // REFLECTION
                .register_type::<Replicate>()
                // PLUGIN
                .add_plugins(ReplicationSendPlugin::<ConnectionManager>::new(
                    self.tick_interval,
                    send_interval,
                ))
                // SETS
                .configure_sets(
                    PostUpdate,
                    // only replicate entities once client is synced
                    // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
                    //  and the message might be ignored by the server
                    //  But then pre-predicted entities that are spawned right away will not be replicated?
                    // NOTE: we always need to add this condition if we don't enable replication, because
                    InternalReplicationSet::<ClientMarker>::All
                        .run_if(is_connected.and(is_synced).and(not(is_host_server))),
                )
                // SYSTEMS
                .add_systems(
                    PostUpdate,
                    (
                        buffer_replication_messages
                            .in_set(InternalReplicationSet::<ClientMarker>::AfterBuffer),
                        add_replicated_component_host_server.run_if(is_host_server),
                    ),
                );

            // TODO: since we use observers, we could buffer a component add/remove/add within a single replication interval!
            //  need to use a hashmap in the buffer logic to have only a single add or remove..
            //  if we have an Add and we already had buffered a remove, keep only the Add (because at the time of sending,
            //    the component is there)
            // TODO: or maybe don't use observers for buffering component removes..
            app.add_observer(send_entity_despawn);
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
                    // Or<(With<ReplicateLike>, (With<Replicating>, With<ReplicateToClient>, With<HasAuthority>))>
                    builder.or(|b| {
                        b.with::<ReplicateLike>();
                        b.and(|b| {
                            b.with::<Replicating>();
                            b.with::<ReplicateToServer>();
                            b.with::<HasAuthority>();
                        });
                    });
                    builder.optional(|b| {
                        b.data::<(
                            &ReplicateLike,
                            &ReplicateToServer,
                            &ReplicationGroup,
                            &TargetEntity,
                            &DisabledComponents,
                            &DeltaCompression,
                            &ReplicateOnce,
                        )>();
                        // include access to &C for all replication components with the right direction
                        component_registry
                            .replication_map
                            .iter()
                            .filter(|(_, m)| m.direction != ChannelDirection::ServerToClient)
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
                replicate
                    .in_set(InternalReplicationSet::<ClientMarker>::BufferEntityUpdates)
                    .in_set(InternalReplicationSet::<ClientMarker>::BufferComponentUpdates),
            );

            app.world_mut().insert_resource(component_registry);
        }
    }

    /// Marker component that indicates that the entity should be replicated to the server
    ///
    /// If this component gets removed, we despawn the entity on the server.
    #[derive(Component, Clone, Copy, Default, Debug, PartialEq, Reflect)]
    #[reflect(Component)]
    #[require(HasAuthority, ReplicationMarker)]
    pub struct ReplicateToServer;

    /// Bundle that indicates how an entity should be replicated. Add this to an entity to start replicating
    /// it to the server.
    ///
    /// ```rust
    /// use bevy::prelude::*;
    /// use lightyear::prelude::*;
    /// use lightyear::prelude::client::Replicate;
    ///
    /// let mut world = World::default();
    /// world.spawn(Replicate::default());
    /// ```
    ///
    /// The bundle is composed of several components:
    /// - [`ReplicateToServer`] to specify if the entity should be replicated to the server or not
    /// - [`ReplicationGroup`] to group entities together for replication. Entities in the same group
    ///   will be sent together in the same message.
    #[derive(Bundle, Clone, Default, PartialEq, Debug, Reflect)]
    pub struct Replicate {
        /// Marker indicating that the entity should be replicated to the server.
        /// If this component is removed, the entity will be despawned on the server.
        pub target: ReplicateToServer,
        /// Marker component that indicates that the client has authority over the entity.
        /// This means that this client:
        /// - is allowed to send replication updates for this entity
        /// - will not accept any replication messages for this entity
        pub authority: HasAuthority,
        /// The replication group defines how entities are grouped (sent as a single message) for replication.
        ///
        /// After the entity is first replicated, the replication group of the entity should not be modified.
        /// (but more entities can be added to the replication group)
        // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
        //  it just keeps living but doesn't receive any updates. Should we make this configurable?
        pub group: ReplicationGroup,
        /// Marker indicating that we should send replication updates for that entity
        /// If this entity is removed, we pause replication for that entity.
        /// (but the entity is not despawned on the server)
        pub replicating: Replicating,
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
    }

    // TODO: implement this with observers, OnAdd<ReplicateToServer>
    // TODO: we should still emit ComponentSpawnEvent, etc. on the server?
    /// In HostServer mode, we will add the [`Replicated`] component to the client->server replicated entities
    /// so that the server can query them using the [`Replicated`] component
    fn add_replicated_component_host_server(
        mut commands: Commands,
        query: Query<
            Entity,
            (
                With<Replicating>,
                With<ReplicateToServer>,
                Without<Replicated>,
            ),
        >,
        connection: Res<ClientConnection>,
    ) {
        let local_client = connection.id();
        for entity in query.iter() {
            commands.entity(entity).insert((
                Replicated {
                    from: Some(local_client),
                },
                InitialReplicated {
                    from: Some(local_client),
                },
            ));
        }
    }

    pub(crate) fn replicate(
        // query &C + various replication components
        query: Query<FilteredEntityRef>,
        tick_manager: Res<TickManager>,
        component_registry: Res<ComponentRegistry>,
        system_ticks: SystemChangeTick,
        mut sender: ResMut<ConnectionManager>,
        archetypes: &Archetypes,
        components: &Components,
        mut replicated_archetypes: Local<ClientReplicatedArchetypes>,
    ) {
        replicated_archetypes.update(archetypes, components, component_registry.as_ref());

        // TODO: skip disabled entities?
        query.iter().for_each(|entity_ref| {
            let entity = entity_ref.id();

            let (
                group_id,
                priority,
                group_ready,
                target_entity,
                disabled_components,
                delta_compression,
                replicate_once,
                replication_target_ticks,
                is_replicate_like_added,
            ) = match entity_ref.get::<ReplicateLike>() { Some(replicate_like) => {
                // root entity does not exist
                let Ok(root_entity_ref) = query.get(replicate_like.0) else {
                    return;
                };
                if root_entity_ref.get::<ReplicateToServer>().is_none() {
                    // ReplicateLike points to a parent entity that doesn't have ReplicationToServer, skip
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
                        .get::<TargetEntity>()
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
                    // SAFETY: we know that the entity has the ReplicateToServer component
                    // because the archetype is in replicated_archetypes
                    unsafe {
                        entity_ref
                            .get_change_ticks::<ReplicateToServer>()
                            .or_else(|| root_entity_ref.get_change_ticks::<ReplicateToServer>())
                            .unwrap_unchecked()
                    },
                    unsafe {
                        entity_ref
                            .get_change_ticks::<ReplicateLike>()
                            .unwrap_unchecked()
                            .is_changed(system_ticks.last_run(), system_ticks.this_run())
                    },
                )
            } _ => {
                let (group_id, priority, group_ready) = entity_ref
                    .get::<ReplicationGroup>()
                    .map(|g| (g.group_id(Some(entity)), g.priority(), g.should_send))
                    .unwrap();
                (
                    group_id,
                    priority,
                    group_ready,
                    entity_ref.get::<TargetEntity>(),
                    entity_ref.get::<DisabledComponents>(),
                    entity_ref.get::<DeltaCompression>(),
                    entity_ref.get::<ReplicateOnce>(),
                    // SAFETY: we know that the entity has the ReplicateToServer component
                    // because the archetype is in replicated_archetypes
                    unsafe {
                        entity_ref
                            .get_change_ticks::<ReplicateToServer>()
                            .unwrap_unchecked()
                    },
                    false,
                )
            }};

            // the update will be 'insert' instead of update if the ReplicateToServer component is new
            // or the HasAuthority component is new. That's because the remote cannot receive update
            // without receiving an action first (to populate the latest_tick on the replication-receiver)
            let replication_is_changed = replication_target_ticks
                .is_changed(system_ticks.last_run(), system_ticks.this_run());

            // TODO: do the entity mapping here!

            // b. add entity despawns from ReplicateToServer component being removed
            // replicate_entity_despawn(
            //     entity.id(),
            //     group_id,
            //     &replication_target,
            //     visibility,
            //     &mut sender,
            // );

            // c. add entity spawns
            // we never want to send a spawn if the entity was replicated to us from the server
            // (because the server already has the entity)
            if replication_is_changed || is_replicate_like_added {
                replicate_entity_spawn(entity, group_id, priority, target_entity, &mut sender);
            }

            // If the group is not set to send, skip this entity
            if !group_ready {
                return;
            }

            // TODO: should we pre-cache the list of components to replicate per archetype?
            // d. all components that were added or changed and that are not disabled

            // NOTE: we pre-cache the list of components for each archetype to not iterate through
            //  all replicated components every time
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

                // TODO: maybe the old method was faster because we had-precached the delta-compression data
                //  for the archetype?
                let delta_compression = delta_compression.is_some_and(|d| d.enabled_kind(*kind));
                let replicate_once = replicate_once.is_some_and(|r| r.enabled_kind(*kind));
                let _ = replicate_component_update(
                    tick_manager.tick(),
                    &component_registry,
                    entity,
                    *kind,
                    data,
                    component_ticks,
                    replication_is_changed,
                    group_id,
                    delta_compression,
                    replicate_once,
                    &system_ticks,
                    &mut sender,
                )
                .inspect_err(|e| {
                    error!(
                        "Error replicating component {:?} update for entity {:?}: {:?}",
                        kind, entity, e
                    )
                });
            }
        })
    }

    /// Send entity spawn replication messages to server when the ReplicationTarget component is added
    /// Also handles:
    /// - handles TargetEntity if it's a Preexisting entity
    /// - setting the priority
    pub(crate) fn replicate_entity_spawn(
        entity: Entity,
        group_id: ReplicationGroupId,
        priority: f32,
        target_entity: Option<&TargetEntity>,
        sender: &mut ConnectionManager,
    ) {
        // // if we had this entity mapped, that means it already exists on the server
        // if let Some(remote_entity) = sender
        //     .replication_receiver
        //     .remote_entity_map
        //     .local_to_remote
        //     .get(&entity)
        // {
        //     return;
        // }
        debug!(?entity, "Prepare entity spawn to server");
        if let Some(TargetEntity::Preexisting(remote_entity)) = target_entity {
            sender
                .replication_sender
                .prepare_entity_spawn_reuse(entity, group_id, *remote_entity);
        } else {
            sender
                .replication_sender
                .prepare_entity_spawn(entity, group_id);
        }
        // also set the priority for the group when we spawn it
        sender
            .replication_sender
            .update_base_priority(group_id, priority);
    }

    /// Send entity despawn if:
    /// - an entity with Replicating had the ReplicationToServerTarget removed
    /// - an entity is despawned
    pub(crate) fn send_entity_despawn(
        // this covers both cases
        trigger: Trigger<OnRemove, (ReplicateToServer, ReplicateLike)>,
        root_query: Query<&ReplicateLike>,
        // only replicate the despawn event if the entity still has Replicating at the time of despawn
        // TODO: but how do we detect if both Replicating AND ReplicateToServer are removed at the same time?
        //  in which case we don't want to replicate the despawn.
        //  i.e. if a user wants to despawn an entity without replicating the despawn
        //  I guess we can provide a command that first removes Replicating, and then despawns the entity.
        query: Query<&ReplicationGroup, With<Replicating>>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        let mut entity = trigger.target();
        let root = root_query.get(entity).map_or(entity, |r| r.0);
        // TODO: use the child's ReplicationGroup if there is one that overrides the root's
        if let Ok(group) = query.get(root) {
            // convert the entity to a network entity (possibly mapped)
            entity = sender
                .replication_receiver
                .remote_entity_map
                .to_remote(entity);
            trace!(?entity, "send entity despawn");
            sender
                .replication_sender
                .prepare_entity_despawn(entity, group.group_id(Some(entity)));
        };
    }

    /// This system sends updates for all components that were added or changed
    /// Sends both ComponentInsert for newly added components and ComponentUpdates otherwise
    ///
    /// Updates are sent only for any components that were changed since the most recent of:
    /// - last time we sent an update for that group which got acked.
    ///
    /// NOTE: cannot use ConnectEvents because they are reset every frame
    fn replicate_component_update(
        current_tick: Tick,
        component_registry: &ComponentRegistry,
        mut entity: Entity,
        component_kind: ComponentKind,
        component_data: Ptr,
        component_ticks: ComponentTicks,
        force_insert: bool,
        group_id: ReplicationGroupId,
        delta_compression: bool,
        replicate_once: bool,
        system_ticks: &SystemChangeTick,
        sender: &mut ConnectionManager,
    ) -> Result<(), ReplicationError> {
        let (mut insert, mut update) = (false, false);

        // send a component_insert for components that were newly added
        // or if we start replicating the entity
        // TODO: ideally we would use target.is_added(), but we do the trick of setting all the
        //  ReplicateToServer components to `changed` when the client first connects so that we replicate existing entities to the server
        //  That is why `force_insert = True` if ReplicateToServer is changed.
        if component_ticks.is_added(system_ticks.last_run(), system_ticks.this_run())
            || force_insert
        {
            trace!("component is added or replication_target is added");
            insert = true;
        } else {
            // do not send updates for these components, only inserts/removes
            if replicate_once {
                trace!(
                    ?entity,
                    "not replicating updates for {:?} because it is marked as replicate_once",
                    component_kind
                );
                return Ok(());
            }
            // otherwise send an update for all components that changed since the
            // last update we have ack-ed
            update = true;
        }
        if insert || update {
            // convert the entity to a network entity (possibly mapped)
            entity = sender
                .replication_receiver
                .remote_entity_map
                .to_remote(entity);

            let writer = &mut sender.writer;
            if insert {
                trace!(?entity, "send insert");
                if delta_compression {
                    // SAFETY: the component_data corresponds to the kind
                    unsafe {
                        component_registry.serialize_diff_from_base_value(
                            component_data,
                            writer,
                            component_kind,
                            &mut sender
                                .replication_receiver
                                .remote_entity_map
                                .local_to_remote,
                        )?
                    }
                } else {
                    component_registry.erased_serialize(
                        component_data,
                        writer,
                        component_kind,
                        &mut sender
                            .replication_receiver
                            .remote_entity_map
                            .local_to_remote,
                    )?;
                };
                let raw_data = writer.split();
                sender
                    .replication_sender
                    .prepare_component_insert(entity, group_id, raw_data);
            } else {
                trace!(?entity, "send update");
                let send_tick = sender
                    .replication_sender
                    .group_channels
                    .entry(group_id)
                    .or_default()
                    .send_tick;

                // send the update for all changes newer than the last send bevy tick for the group
                if send_tick.map_or(true, |c| {
                    component_ticks.is_changed(c, system_ticks.this_run())
                }) {
                    trace!(
                        change_tick = ?component_ticks.changed,
                        ?send_tick,
                        current_tick = ?system_ticks.this_run(),
                        "prepare entity update changed check"
                    );
                    // trace!(
                    //     ?entity,
                    //     component = ?kind,
                    //     tick = ?self.tick_manager.tick(),
                    //     "Updating single component"
                    // );
                    if delta_compression {
                        sender.replication_sender.prepare_delta_component_update(
                            entity,
                            group_id,
                            component_kind,
                            component_data,
                            component_registry,
                            writer,
                            &mut sender.delta_manager,
                            current_tick,
                            &mut sender.replication_receiver.remote_entity_map,
                        )?;
                    } else {
                        component_registry.erased_serialize(
                            component_data,
                            writer,
                            component_kind,
                            &mut sender
                                .replication_receiver
                                .remote_entity_map
                                .local_to_remote,
                        )?;
                        let raw_data = writer.split();
                        sender
                            .replication_sender
                            .prepare_component_update(entity, group_id, raw_data);
                    }
                }
            }
        }
        Ok(())
    }

    /// Send component remove message when a component gets removed
    pub(crate) fn send_component_removed<C: Component>(
        trigger: Trigger<OnRemove, C>,
        registry: Res<ComponentRegistry>,
        mut sender: ResMut<ConnectionManager>,
        root_query: Query<&ReplicateLike>,
        // only remove the component for entities that are being actively replicated
        query: Query<
            (&ReplicationGroup, Option<&DisabledComponents>),
            (With<Replicating>, With<ReplicateToServer>),
        >,
    ) {
        let mut entity = trigger.target();
        let root = root_query.get(entity).map_or(entity, |r| r.0);
        // TODO: be able to override the root components with those from the child
        if let Ok((group, disabled_components)) = query.get(root) {
            // convert the entity to a network entity (possibly mapped)
            entity = sender
                .replication_receiver
                .remote_entity_map
                .to_remote(entity);
            // do not replicate components (even removals) that are disabled
            if disabled_components
                .is_some_and(|disabled_components| !disabled_components.enabled::<C>())
            {
                return;
            }
            let group_id = group.group_id(Some(root));
            trace!(?entity, kind = ?core::any::type_name::<C>(), "Sending RemoveComponent");
            let kind = registry.net_id::<C>();
            sender
                .replication_sender
                .prepare_component_remove(entity, group_id, kind);
        }
    }

    pub(crate) fn register_replicate_component_send<C: Component>(app: &mut App) {
        // TODO: what if we remove and add within one replication_interval?
        app.add_observer(send_component_removed::<C>);
    }

    #[cfg(test)]
    mod tests {
        use crate::client::replication::send::ReplicateToServer;
        use crate::prelude::{
            server, ChannelDirection, ClientId, ComponentRegistry, DisabledComponents,
            ReplicateOnce, Replicated, TargetEntity,
        };
        use crate::protocol::component::ComponentKind;
        use crate::tests::protocol::{ComponentSyncModeFull, ComponentSyncModeOnce};
        use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
        #[cfg(not(feature = "std"))]
        use alloc::vec;
        use bevy::prelude::ChildOf;

        #[test]
        fn test_entity_spawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server with visibility::All
            let client_entity = stepper.client_app.world_mut().spawn_empty().id();
            let client_child = stepper
                .client_app
                .world_mut()
                .spawn(ChildOf(client_entity))
                .id();
            stepper.frame_step();
            stepper.frame_step();

            // add replicate
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert(ReplicateToServer);
            // TODO: we need to run a couple frames because the server doesn't read the client's updates
            //  because they are from the future
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .expect("client connection missing")
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to server");
            stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .expect("client connection missing")
                .replication_receiver
                .remote_entity_map
                .get_local(client_child)
                .expect("entity was not replicated to server");
        }

        /// Check that when an entity is already replicated and you add a Child to it
        /// the child also gets replicated
        #[test]
        fn test_entity_spawn_child() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server with visibility::All
            let client_entity = stepper.client_app.world_mut().spawn(ReplicateToServer).id();

            stepper.frame_step();
            stepper.frame_step();

            // add replicate
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert(ReplicateToServer);
            // TODO: we need to run a couple frames because the server doesn't read the client's updates
            //  because they are from the future
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .expect("client connection missing")
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to server");

            // Add a child
            let client_child = stepper
                .client_app
                .world_mut()
                .spawn(ChildOf(client_entity))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }
            stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .expect("client connection missing")
                .replication_receiver
                .remote_entity_map
                .get_local(client_child)
                .expect("entity was not replicated to server");
        }

        #[test]
        fn test_multi_entity_spawn() {
            let mut stepper = BevyStepper::default();
            let server_entities = stepper.server_app.world().entities().len();

            // spawn an entity on server
            stepper
                .client_app
                .world_mut()
                .spawn_batch(vec![ReplicateToServer; 2]);
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entities were spawned
            assert_eq!(
                stepper.server_app.world().entities().len(),
                server_entities + 2
            );
        }

        #[test]
        fn test_entity_spawn_preexisting_target() {
            let mut stepper = BevyStepper::default();

            let server_entity = stepper.server_app.world_mut().spawn_empty().id();
            stepper.frame_step();
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn((ReplicateToServer, TargetEntity::Preexisting(server_entity)))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was replicated on the server entity
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .resource::<server::ConnectionManager>()
                    .connection(ClientId::Netcode(TEST_CLIENT_ID))
                    .unwrap()
                    .replication_receiver
                    .remote_entity_map
                    .get_local(client_entity)
                    .unwrap(),
                server_entity
            );
            assert!(stepper
                .server_app
                .world()
                .get::<Replicated>(server_entity)
                .is_some());
        }

        /// Check that if we remove ReplicationToServer
        /// the entity gets despawned on the server
        #[test]
        fn test_entity_spawn_replication_target_update() {
            let mut stepper = BevyStepper::default();

            let client_entity = stepper.client_app.world_mut().spawn_empty().id();
            stepper.frame_step();
            stepper.frame_step();

            // add replicate
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert(ReplicateToServer);
            // TODO: we need to run a couple frames because the server doesn't read the client's updates
            //  because they are from the future
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .expect("client connection missing")
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to server");

            // remove the ReplicateToServer component
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .remove::<ReplicateToServer>();
            for _ in 0..10 {
                stepper.frame_step();
            }
            assert!(stepper
                .server_app
                .world()
                .get_entity(server_entity)
                .is_err());
        }

        #[test]
        fn test_entity_despawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper.client_app.world_mut().spawn(ReplicateToServer).id();
            let client_child = stepper
                .client_app
                .world_mut()
                .spawn(ChildOf(client_entity))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");
            let server_child = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_child)
                .expect("entity was not replicated to client");

            // despawn
            stepper.client_app.world_mut().despawn(client_entity);
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was despawned
            assert!(stepper
                .server_app
                .world()
                .get_entity(server_entity)
                .is_err());
            // check that the child was despawned
            assert!(stepper.server_app.world().get_entity(server_child).is_err());
        }

        /// Check that if you despawn an entity with ReplicateLike,
        /// the despawn is replicated
        #[test]
        fn test_entity_despawn_child() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper.client_app.world_mut().spawn(ReplicateToServer).id();
            let client_child = stepper
                .client_app
                .world_mut()
                .spawn(ChildOf(client_entity))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_child = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_child)
                .expect("entity was not replicated to client");

            // despawn
            stepper.client_app.world_mut().despawn(client_child);
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the child was despawned
            assert!(stepper.server_app.world().get_entity(server_child).is_err());
        }

        #[test]
        fn test_component_insert() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper.client_app.world_mut().spawn(ReplicateToServer).id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert(ComponentSyncModeFull(1.0));
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the component was replicated
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .entity(server_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("Component missing"),
                &ComponentSyncModeFull(1.0)
            )
        }

        #[test]
        fn test_component_insert_disabled() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper.client_app.world_mut().spawn(ReplicateToServer).id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");

            // add component
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert((
                    ComponentSyncModeFull(1.0),
                    DisabledComponents::default().disable::<ComponentSyncModeFull>(),
                ));
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the component was not  replicated
            assert!(stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<ComponentSyncModeFull>()
                .is_none());
        }

        // TODO: check that component insert for a component that doesn't have ClientToServer is not replicated!

        #[test]
        fn test_component_update() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn((ReplicateToServer, ComponentSyncModeFull(1.0)))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");

            // update component
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert(ComponentSyncModeFull(2.0));
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the component was updated
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .entity(server_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("Component missing"),
                &ComponentSyncModeFull(2.0)
            )
        }

        // TODO: hard to test because we need to wait a few ticks on the server..
        //  maybe disable sync for tests?
        // #[test]
        // fn test_component_update_send_frequency() {
        //     let mut stepper = BevyStepper::default();
        //
        //     // spawn an entity on server
        //     let client_entity = stepper
        //         .client_app
        //         .world
        //         .spawn((
        //             Replicate {
        //                 // replicate every 4 ticks
        //                 group: ReplicationGroup::new_from_entity()
        //                     .set_send_frequency(Duration::from_millis(40)),
        //                 ..default()
        //             },
        //             Component1(1.0),
        //         ))
        //         .id();
        //     stepper.frame_step();
        //     stepper.frame_step();
        //     let server_entity = *stepper
        //         .server_app
        //         .world
        //         .resource::<server::ConnectionManager>()
        //         .connection(ClientId::Netcode(TEST_CLIENT_ID))
        //         .unwrap()
        //         .replication_receiver
        //         .remote_entity_map
        //         .get_local(client_entity)
        //         .expect("entity was not replicated to client");
        //
        //     // update component
        //     stepper
        //         .client_app
        //         .world
        //         .entity_mut(client_entity)
        //         .insert(Component1(2.0));
        //     stepper.frame_step();
        //     stepper.frame_step();
        //
        //     // check that the component was not updated (because it had been only three ticks)
        //     assert_eq!(
        //         stepper
        //             .server_app
        //             .world
        //             .entity(server_entity)
        //             .get::<Component1>()
        //             .expect("component missing"),
        //         &Component1(1.0)
        //     );
        //     // it has been 4 ticks, the component was updated
        //     stepper.frame_step();
        //     // check that the component was not updated (because it had been only two ticks)
        //     assert_eq!(
        //         stepper
        //             .server_app
        //             .world
        //             .entity(server_entity)
        //             .get::<Component1>()
        //             .expect("component missing"),
        //         &Component1(2.0)
        //     );
        // }

        #[test]
        fn test_component_update_disabled() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn((ReplicateToServer, ComponentSyncModeFull(1.0)))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .entity(server_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("Component missing"),
                &ComponentSyncModeFull(1.0)
            );

            // remove component
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .remove::<ComponentSyncModeFull>();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the component was removed
            assert!(stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<ComponentSyncModeFull>()
                .is_none());
        }

        #[test]
        fn test_component_update_replicate_once() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn((
                    ReplicateToServer,
                    ComponentSyncModeFull(1.0),
                    ReplicateOnce::default().add::<ComponentSyncModeFull>(),
                ))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");

            // update component
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert(ComponentSyncModeFull(2.0));
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the component was not updated
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .entity(server_entity)
                    .get::<ComponentSyncModeFull>()
                    .expect("Component missing"),
                &ComponentSyncModeFull(1.0)
            )
        }

        #[test]
        fn test_component_remove() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn((ReplicateToServer, ComponentSyncModeFull(1.0)))
                .id();
            let client_child = stepper
                .client_app
                .world_mut()
                .spawn((
                    ChildOf(client_entity),
                    ComponentSyncModeFull(1.0),
                ))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");
            let server_child = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_child)
                .expect("entity was not replicated to client");

            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .entity(server_entity)
                    .get::<ComponentSyncModeFull>(),
                Some(&ComponentSyncModeFull(1.0))
            );
            assert_eq!(
                stepper
                    .server_app
                    .world()
                    .entity(server_child)
                    .get::<ComponentSyncModeFull>(),
                Some(&ComponentSyncModeFull(1.0))
            );

            // remove component on the parent and the child
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .remove::<ComponentSyncModeFull>();
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_child)
                .remove::<ComponentSyncModeFull>();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the component was removed
            assert!(stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<ComponentSyncModeFull>()
                .is_none());
            assert!(stepper
                .server_app
                .world()
                .entity(server_child)
                .get::<ComponentSyncModeFull>()
                .is_none());
        }

        /// Make sure that ServerToClient components are not replicated to the server
        #[test]
        fn test_component_direction() {
            let mut stepper = BevyStepper::default();

            assert_eq!(
                stepper
                    .client_app
                    .world()
                    .resource::<ComponentRegistry>()
                    .direction(ComponentKind::of::<ComponentSyncModeOnce>()),
                Some(ChannelDirection::ServerToClient)
            );

            // spawn an entity on client
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn((ReplicateToServer, ComponentSyncModeOnce(1.0)))
                .id();
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            let server_entity = stepper
                .server_app
                .world()
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to client");
            // check that the component was not replicated to the server
            assert!(stepper
                .server_app
                .world()
                .get::<ComponentSyncModeOnce>(server_entity)
                .is_none());
        }
    }
}

pub(crate) mod commands {
    use crate::prelude::Replicating;
    use bevy::ecs::system::EntityCommands;
    use bevy::prelude::EntityWorldMut;

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
}
