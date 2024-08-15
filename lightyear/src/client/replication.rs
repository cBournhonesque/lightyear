//! Client replication plugins
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::connection::ConnectionManager;
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

pub(crate) mod receive {
    use super::*;
    use crate::prelude::client::MessageEvent;
    use crate::prelude::{
        client::{is_connected, is_synced},
        is_host_server,
    };
    use crate::shared::replication::authority::{AuthorityChange, HasAuthority};
    use crate::shared::sets::InternalMainSet;

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
                InternalReplicationSet::<ClientMarker>::All.run_if(
                    is_connected
                        .and_then(is_synced)
                        .and_then(not(is_host_server)),
                ),
            );

            app.add_systems(
                PreUpdate,
                handle_authority_change.after(InternalMainSet::<ClientMarker>::EmitEvents),
            );
        }
    }

    /// Apply authority changes requested by the server
    // TODO: use observer to handle these?
    fn handle_authority_change(
        mut commands: Commands,
        mut messages: ResMut<Events<MessageEvent<AuthorityChange>>>,
    ) {
        for message in messages.drain() {
            let entity = message.message.entity;
            if let Some(mut entity_mut) = commands.get_entity(entity) {
                if message.message.gain_authority {
                    entity_mut.insert(HasAuthority);
                } else {
                    entity_mut.remove::<HasAuthority>();
                }
            }
        }
    }
}

pub(crate) mod send {
    use super::*;
    use bevy::ecs::component::ComponentTicks;

    use crate::connection::client::ClientConnection;

    use crate::prelude::client::{ClientConfig, NetClient};

    use crate::prelude::{
        client::{is_connected, is_synced},
        is_host_server, ComponentRegistry, DisabledComponent, RemoteEntityMap, ReplicateHierarchy,
        Replicated, ReplicationGroup, TargetEntity, Tick, TickManager, TimeManager,
    };
    use crate::protocol::component::ComponentKind;

    use crate::shared::replication::components::{Replicating, ReplicationGroupId};

    use crate::shared::replication::archetypes::{
        get_erased_component, ClientReplicatedArchetypes,
    };
    use crate::shared::replication::authority::HasAuthority;
    use crate::shared::replication::error::ReplicationError;
    use bevy::ecs::system::SystemChangeTick;
    use bevy::ptr::Ptr;

    #[derive(Default)]
    pub struct ClientReplicationSendPlugin {
        pub tick_interval: Duration,
    }

    impl Plugin for ClientReplicationSendPlugin {
        fn build(&self, app: &mut App) {
            let send_interval = app
                .world()
                .resource::<ClientConfig>()
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
                            .and_then(is_synced)
                            .and_then(not(is_host_server)),
                    ),
                )
                // SYSTEMS
                .add_systems(
                    PostUpdate,
                    (
                        replicate
                            .in_set(InternalReplicationSet::<ClientMarker>::BufferEntityUpdates)
                            .in_set(InternalReplicationSet::<ClientMarker>::BufferComponentUpdates),
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
            app.observe(send_entity_despawn);
        }
    }

    /// Marker component that indicates that the entity should be replicated to the server
    ///
    /// If this component gets removed, we despawn the entity on the server.
    #[derive(Component, Clone, Copy, Default, Debug, PartialEq, Reflect)]
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
    /// - [`ReplicateHierarchy`] to specify how the hierarchy of the entity should be replicated
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
        /// How should the hierarchy of the entity (parents/children) be replicated?
        pub hierarchy: ReplicateHierarchy,
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
            commands.entity(entity).insert(Replicated {
                from: Some(local_client),
            });
        }
    }

    pub(crate) fn replicate(
        tick_manager: Res<TickManager>,
        component_registry: Res<ComponentRegistry>,
        mut replicated_archetypes: Local<ClientReplicatedArchetypes>,
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
                let target_entity = entity_ref.get::<TargetEntity>();
                // SAFETY: we know that the entity has the ReplicationTarget component
                // because the archetype is in replicated_archetypes
                let replication_target_ticks = unsafe {
                    entity_ref
                        .get_change_ticks::<ReplicateToServer>()
                        .unwrap_unchecked()
                };
                let has_authority_ticks = unsafe {
                    entity_ref
                        .get_change_ticks::<HasAuthority>()
                        .unwrap_unchecked()
                };
                // the update will be 'insert' instead of update if the ReplicateToServer component is new
                // or the HasAuthority component is new. That's because the remote cannot receive update
                // without receiving an action first (to populate the latest_tick)
                let replication_is_changed = replication_target_ticks
                    .is_changed(system_ticks.last_run(), system_ticks.this_run());
                let has_authority_is_changed =
                    has_authority_ticks.is_added(system_ticks.last_run(), system_ticks.this_run());

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
                if replication_is_changed {
                    replicate_entity_spawn(
                        entity.id(),
                        group_id,
                        priority,
                        target_entity,
                        &mut sender,
                    );
                }

                // If the group is not set to send, skip this entity
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
                    let _ = replicate_component_update(
                        tick_manager.tick(),
                        &component_registry,
                        entity.id(),
                        replicated_component.kind,
                        data,
                        component_ticks,
                        replication_is_changed || has_authority_is_changed,
                        group_id,
                        replicated_component.delta_compression,
                        replicated_component.replicate_once,
                        &system_ticks,
                        &mut sender,
                    )
                    .inspect_err(|e| {
                        error!(
                            "Error replicating component {:?} update for entity {:?}: {:?}",
                            replicated_component.kind,
                            entity.id(),
                            e
                        )
                    });
                }
            }
        }

        // restore the ConnectionManager
        *set.p1() = sender;
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
        info!(?entity, "Prepare entity spawn to server");
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
        trigger: Trigger<OnRemove, ReplicateToServer>,
        // only replicate the despawn event if the entity still has Replicating at the time of despawn
        // TODO: but how do we detect if both Replicating AND ReplicateToServer are removed at the same time?
        //  in which case we don't want to replicate the despawn.
        //  i.e. if a user wants to despawn an entity without replicating the despawn
        //  I guess we can provide a command that first removes Replicating, and then despawns the entity.
        query: Query<&ReplicationGroup, With<Replicating>>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        let mut entity = trigger.entity();
        // convert the entity to a network entity (possibly mapped)
        entity = sender
            .replication_receiver
            .remote_entity_map
            .to_remote(entity);
        if let Ok(group) = query.get(entity) {
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
                info!(?entity, "send insert");
                if delta_compression {
                    // SAFETY: the component_data corresponds to the kind
                    unsafe {
                        component_registry.serialize_diff_from_base_value(
                            component_data,
                            writer,
                            component_kind,
                        )?
                    }
                } else {
                    component_registry.erased_serialize(component_data, writer, component_kind)?;
                };
                let raw_data = writer.split();
                sender.replication_sender.prepare_component_insert(
                    entity,
                    group_id,
                    raw_data,
                    system_ticks.this_run(),
                );
            } else {
                info!(?entity, "send update");
                let send_tick = sender
                    .replication_sender
                    .group_channels
                    .entry(group_id)
                    .or_default()
                    .send_tick;

                // send the update for all changes newer than the last send bevy tick for the group
                if send_tick.map_or(true, |c| {
                    component_ticks
                        .last_changed_tick()
                        .is_newer_than(c, system_ticks.this_run())
                }) {
                    trace!(
                        change_tick = ?component_ticks.last_changed_tick(),
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
                        )?;
                    } else {
                        component_registry.erased_serialize(
                            component_data,
                            writer,
                            component_kind,
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

    /// Send component remove
    pub(crate) fn send_component_removed<C: Component>(
        trigger: Trigger<OnRemove, C>,
        registry: Res<ComponentRegistry>,
        mut sender: ResMut<ConnectionManager>,
        // only remove the component for entities that are being actively replicated
        query: Query<
            (&ReplicationGroup, Has<DisabledComponent<C>>),
            (With<Replicating>, With<ReplicateToServer>),
        >,
    ) {
        let mut entity = trigger.entity();
        // convert the entity to a network entity (possibly mapped)
        entity = sender
            .replication_receiver
            .remote_entity_map
            .to_remote(entity);
        if let Ok((group, disabled)) = query.get(entity) {
            // do not replicate components that are disabled
            if disabled {
                return;
            }
            let group_id = group.group_id(Some(entity));
            trace!(?entity, kind = ?std::any::type_name::<C>(), "Sending RemoveComponent");
            let kind = registry.net_id::<C>();
            sender
                .replication_sender
                .prepare_component_remove(entity, group_id, kind);
        }
    }

    pub(crate) fn register_replicate_component_send<C: Component>(app: &mut App) {
        // TODO: what if we remove and add within one replication_interval?
        app.observe(send_component_removed::<C>);
    }

    #[cfg(test)]
    mod tests {
        use crate::client::replication::send::ReplicateToServer;
        use crate::prelude::client::Replicate;
        use crate::prelude::{
            server, ClientId, DisabledComponent, ReplicateOnceComponent, Replicated, TargetEntity,
        };
        use crate::tests::protocol::ComponentSyncModeFull;
        use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};

        #[test]
        fn test_entity_spawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server with visibility::All
            let client_entity = stepper.client_app.world_mut().spawn_empty().id();
            stepper.frame_step();
            stepper.frame_step();

            // add replicate
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert(Replicate::default());
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
        }

        #[test]
        fn test_multi_entity_spawn() {
            let mut stepper = BevyStepper::default();
            let server_entities = stepper.server_app.world().entities().len();

            // spawn an entity on server
            stepper
                .client_app
                .world_mut()
                .spawn_batch(vec![Replicate::default(); 2]);
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
                .spawn((
                    Replicate::default(),
                    TargetEntity::Preexisting(server_entity),
                ))
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
                .insert(Replicate::default());
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
                .is_none());
        }

        #[test]
        fn test_entity_despawn() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn(Replicate::default())
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
                .is_none());
        }

        #[test]
        fn test_component_insert() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on client
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn(Replicate::default())
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
            let client_entity = stepper
                .client_app
                .world_mut()
                .spawn(Replicate::default())
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

            // add component
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_entity)
                .insert((
                    ComponentSyncModeFull(1.0),
                    DisabledComponent::<ComponentSyncModeFull>::default(),
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
                .spawn((Replicate::default(), ComponentSyncModeFull(1.0)))
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
                .spawn((Replicate::default(), ComponentSyncModeFull(1.0)))
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
                    Replicate::default(),
                    ComponentSyncModeFull(1.0),
                    ReplicateOnceComponent::<ComponentSyncModeFull>::default(),
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
                .spawn((Replicate::default(), ComponentSyncModeFull(1.0)))
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
                .insert((
                    ComponentSyncModeFull(2.0),
                    DisabledComponent::<ComponentSyncModeFull>::default(),
                ));
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
    }
}

pub(crate) mod commands {
    use crate::prelude::Replicating;
    use bevy::ecs::system::EntityCommands;
    use bevy::prelude::{Entity, World};

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
}
