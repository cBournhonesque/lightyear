//! Client replication plugins
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::connection::ConnectionManager;
use crate::client::sync::client_is_synced;
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

pub(crate) mod receive {
    use super::*;
    use crate::prelude::{is_connected, is_host_server};
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
                        .and_then(not(is_host_server)),
                ),
            );
        }
    }
}

pub(crate) mod send {
    use super::*;

    use crate::connection::client::ClientConnection;

    use crate::prelude::client::NetClient;

    use crate::prelude::{
        is_connected, is_host_server, ComponentRegistry, DisabledComponent, ReplicateHierarchy,
        ReplicateOnceComponent, Replicated, ReplicationGroup, TargetEntity, TickManager,
    };
    use crate::protocol::component::ComponentKind;

    use crate::shared::replication::components::{DeltaCompression, DespawnTracker, Replicating};

    use crate::serialize::writer::Writer;
    use bevy::ecs::entity::Entities;
    use bevy::ecs::system::SystemChangeTick;
    use bevy::ptr::Ptr;

    #[derive(Default)]
    pub struct ClientReplicationSendPlugin {
        pub tick_interval: Duration,
    }

    impl Plugin for ClientReplicationSendPlugin {
        fn build(&self, app: &mut App) {
            app
                // REFLECTION
                .register_type::<Replicate>()
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
                            .and_then(not(is_host_server)),
                    ),
                )
                // SYSTEMS
                .add_systems(
                    PostUpdate,
                    (
                        // NOTE: we need to run `send_entity_despawn` once per frame (and not once per send_interval)
                        //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
                        //  It is ok to run it every frame because it creates at most one message per despawn
                        // NOTE: we make sure to update the replicate_cache before we make use of it in `send_entity_despawn`
                        handle_replicating_remove
                            .in_set(InternalReplicationSet::<ClientMarker>::BeforeBuffer),
                        send_entity_spawn
                            .in_set(InternalReplicationSet::<ClientMarker>::BufferEntityUpdates),
                        send_entity_despawn.in_set(
                            InternalReplicationSet::<ClientMarker>::BufferDespawnsAndRemovals,
                        ),
                        handle_replicating_add
                            .in_set(InternalReplicationSet::<ClientMarker>::AfterBuffer),
                        add_replicated_component_host_server.run_if(is_host_server),
                    ),
                );
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
    /// will be sent together in the same message.
    /// - [`ReplicateHierarchy`] to specify how the hierarchy of the entity should be replicated
    #[derive(Bundle, Clone, Default, PartialEq, Debug, Reflect)]
    pub struct Replicate {
        /// Marker indicating that the entity should be replicated to the server.
        /// If this component is removed, the entity will be despawned on the server.
        pub target: ReplicateToServer,
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

    // TODO: replace this with observers
    /// Metadata that holds Replicate-information from the previous send_interval's replication.
    /// - when the entity gets despawned, we will use this to know how to replicate the despawn
    #[derive(PartialEq, Debug)]
    pub(crate) struct ReplicateCache {
        pub(crate) replication_group: ReplicationGroup,
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
        query: Query<(Entity, &ReplicationGroup), (With<Replicating>, Without<DespawnTracker>)>,
    ) {
        for (entity, group) in query.iter() {
            debug!("Replicate component was added for entity {entity:?}");
            commands.entity(entity).insert(DespawnTracker);
            sender.replicate_component_cache.insert(
                entity,
                ReplicateCache {
                    replication_group: *group,
                },
            );
        }
    }

    // TODO: implement this with observers, OnAdd<ReplicateToServer>
    /// In HostServer mode, we will add the Replicated component to the client->server replicated entities
    /// so that the server can still react to them using observers.
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

    /// Send entity spawn replication messages to server when the ReplicationTarget component is added
    /// Also handles:
    /// - handles TargetEntity if it's a Preexisting entity
    /// - setting the priority
    pub(crate) fn send_entity_spawn(
        query: Query<
            (Entity, &ReplicationGroup, Option<&TargetEntity>),
            (With<Replicating>, Changed<ReplicateToServer>),
        >,
        mut sender: ResMut<ConnectionManager>,
    ) {
        query.iter().for_each(|(entity, group, target_entity)| {
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

    /// Send entity despawn if:
    /// - an entity that had a DespawnTracker was despawned
    /// - an entity with Replicating had the ReplicationToServerTarget removed
    pub(crate) fn send_entity_despawn(
        query: Query<(Entity, &ReplicationGroup), (With<Replicating>, Without<ReplicateToServer>)>,
        // TODO: ideally we want to send despawns for entities that still had REPLICATE at the time of despawn
        //  not just entities that had despawn tracker once
        mut despawn_removed: RemovedComponents<DespawnTracker>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        // Send entity-despawn for entities that have Replicating
        // but where the `ReplicationToServerTarget` component has been removed
        query.iter().for_each(|(entity, group)| {
            trace!(
                ?entity,
                "send entity despawn because ReplicationToServerTarget was removed"
            );
            sender
                .replication_sender
                .prepare_entity_despawn(entity, group.group_id(Some(entity)));
        });

        // Despawn entities when the entity gets despawned on local world
        for entity in despawn_removed.read() {
            // only replicate the despawn if the entity still had a Replicating component
            if let Some(replicate_cache) = sender.replicate_component_cache.remove(&entity) {
                trace!(
                    ?entity,
                    "send entity despawn because DespawnTracker component was removed"
                );
                sender.replication_sender.prepare_entity_despawn(
                    entity,
                    replicate_cache.replication_group.group_id(Some(entity)),
                );
            }
        }
    }

    /// This system sends updates for all components that were added or changed
    /// Sends both ComponentInsert for newly added components and ComponentUpdates otherwise
    ///
    /// Updates are sent only for any components that were changed since the most recent of:
    /// - last time we sent an update for that group which got acked.
    ///
    /// NOTE: cannot use ConnectEvents because they are reset every frame
    pub(crate) fn send_component_update<C: Component>(
        tick_manager: Res<TickManager>,
        registry: Res<ComponentRegistry>,
        query: Query<
            (
                Entity,
                Ref<C>,
                Ref<ReplicateToServer>,
                &ReplicationGroup,
                Has<DeltaCompression<C>>,
                Has<DisabledComponent<C>>,
                Has<ReplicateOnceComponent<C>>,
            ),
            With<Replicating>,
        >,
        system_bevy_ticks: SystemChangeTick,
        mut connection: ResMut<ConnectionManager>,
    ) {
        let tick = tick_manager.tick();
        let kind = ComponentKind::of::<C>();
        // enable split borrows
        let connection = &mut *connection;
        query.iter().for_each(
            |(entity, component, target, group, delta_compression, disabled, replicate_once)| {
                // do not replicate components that are disabled
                if disabled {
                    return;
                }
                let (mut insert, mut update) = (false, false);

                // send a component_insert for components that were newly added
                // or if we start replicating the entity
                // TODO: ideally we would use target.is_added(), but we do the trick of setting all the
                //  ReplicateToServer components to `changed` on connection so that we replicate existing entities to
                //  the server
                if component.is_added() || target.is_changed() {
                    trace!("component is added or replication_target is added");
                    insert = true;
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
                    update = true;
                }
                if insert || update {
                    let group_id = group.group_id(Some(entity));
                    let component_data = Ptr::from(component.as_ref());
                    let mut writer = Writer::default();
                    if insert {
                        if delta_compression {
                            // SAFETY: the component_data corresponds to the kind
                            unsafe {
                                registry
                                    .serialize_diff_from_base_value(
                                        component_data,
                                        &mut writer,
                                        kind,
                                    )
                                    .expect("could not serialize delta")
                            }
                        } else {
                            registry
                                .erased_serialize(component_data, &mut writer, kind)
                                .expect("could not serialize component")
                        };
                        let raw_data = writer.to_bytes();
                        connection.replication_sender.prepare_component_insert(
                            entity,
                            group_id,
                            raw_data,
                            system_bevy_ticks.this_run(),
                        );
                    } else {
                        // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
                        let send_tick = connection
                            .replication_sender
                            .group_channels
                            .entry(group_id)
                            .or_default()
                            .send_tick;

                        // send the update for all changes newer than the last ack bevy tick for the group
                        if send_tick.map_or(true, |c| {
                            component
                                .last_changed()
                                .is_newer_than(c, system_bevy_ticks.this_run())
                        }) {
                            trace!(
                                change_tick = ?component.last_changed(),
                                ?send_tick,
                                current_tick = ?system_bevy_ticks.this_run(),
                                "prepare entity update changed check"
                            );
                            // trace!(
                            //     ?entity,
                            //     component = ?kind,
                            //     tick = ?self.tick_manager.tick(),
                            //     "Updating single component"
                            // );
                            if !delta_compression {
                                registry
                                    .erased_serialize(component_data, &mut writer, kind)
                                    .expect("could not serialize component");
                                let raw_data = writer.to_bytes();
                                connection
                                    .replication_sender
                                    .prepare_component_update(entity, group_id, raw_data);
                            } else {
                                connection
                                    .replication_sender
                                    .prepare_delta_component_update(
                                        entity,
                                        group_id,
                                        kind,
                                        component_data,
                                        &registry,
                                        &mut writer,
                                        &mut connection.delta_manager,
                                        tick,
                                    );
                            }
                        }
                    }
                }
            },
        );
    }

    /// Send component remove
    pub(crate) fn send_component_removed<C: Component>(
        registry: Res<ComponentRegistry>,
        // only remove the component for entities that are being actively replicated
        query: Query<
            (&ReplicationGroup, Has<DisabledComponent<C>>),
            (With<Replicating>, With<ReplicateToServer>),
        >,
        mut removed: RemovedComponents<C>,
        mut sender: ResMut<ConnectionManager>,
    ) {
        let kind = registry.net_id::<C>();
        removed.read().for_each(|entity| {
            if let Ok((group, disabled)) = query.get(entity) {
                // do not replicate components that are disabled
                if disabled {
                    return;
                }
                let group_id = group.group_id(Some(entity));
                trace!(?entity, kind = ?std::any::type_name::<C>(), "Sending RemoveComponent");
                sender
                    .replication_sender
                    .prepare_component_remove(entity, group_id, kind);
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
                    .in_set(InternalReplicationSet::<ClientMarker>::BufferDespawnsAndRemovals),
                // NOTE: we run this system once every `send_interval` because we don't want to send too many Update messages
                //  and use up all the bandwidth
                send_component_update::<C>
                    .in_set(InternalReplicationSet::<ClientMarker>::BufferComponentUpdates),
            ),
        );
    }

    #[cfg(test)]
    mod tests {
        use crate::prelude::{client, server, ClientId};
        use crate::tests::stepper::{BevyStepper, Step, TEST_CLIENT_ID};

        #[test]
        fn test_entity_spawn_client_to_server() {
            let mut stepper = BevyStepper::default();

            // spawn an entity on server with visibility::All
            let client_entity = stepper.client_app.world.spawn_empty().id();
            stepper.frame_step();
            stepper.frame_step();

            // add replicate
            stepper
                .client_app
                .world
                .entity_mut(client_entity)
                .insert(client::Replicate::default());
            // TODO: we need to run a couple frames because the server doesn't read the client's updates
            //  because they are from the future
            for _ in 0..10 {
                stepper.frame_step();
            }

            // check that the entity was spawned
            stepper
                .server_app
                .world
                .resource::<server::ConnectionManager>()
                .connection(ClientId::Netcode(TEST_CLIENT_ID))
                .expect("client connection missing")
                .replication_receiver
                .remote_entity_map
                .get_local(client_entity)
                .expect("entity was not replicated to server");
        }
    }
}

pub(crate) mod commands {
    use crate::client::connection::ConnectionManager;

    use bevy::ecs::system::EntityCommands;
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
}
