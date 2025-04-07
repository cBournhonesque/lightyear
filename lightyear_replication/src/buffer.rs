//! Client replication plugins
use bevy::prelude::*;
use core::time::Duration;

use super::*;
use bevy::ecs::archetype::Archetypes;
use bevy::ecs::component::{ComponentTicks, Components, HookContext};
use bevy::ecs::entity::{EntityIndexSet, UniqueEntitySlice, UniqueEntityVec};
use bevy::ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
use bevy::ecs::world::{DeferredWorld, FilteredEntityRef};
use bevy::ptr::Ptr;

use crate::archetypes::{ReplicatedArchetypes, ReplicatedComponent};
use crate::components::{DeltaCompression, DisabledComponents, ReplicateOnce, Replicating, ReplicationGroup, ReplicationGroupId, TargetEntity};
use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::hierarchy::ReplicateLike;
use crate::receive::ReplicationReceiver;
use crate::registry::registry::ComponentRegistry;
use crate::registry::ComponentKind;
use crate::send::ReplicationSender;
use lightyear_connection::client::Client;
use lightyear_connection::client_of::{ClientOf, Server};
use lightyear_connection::prelude::NetworkTarget;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{LocalTimeline, NetworkTimeline};
use lightyear_messages::MessageManager;
use lightyear_serde::entity_map::RemoteEntityMap;
use lightyear_transport::prelude::Transport;
use tracing::{debug, error, trace};

#[derive(Clone, Default, Debug, PartialEq, Reflect)]
pub enum ReplicationMode {
    /// Will try to find a single ReplicationSender entity in the world
    #[default]
    SingleSender,
    /// Will try to find a single Client entity in the world
    SingleClient,
    /// Will try to find a single Server entity in the world
    SingleServer(NetworkTarget),
    /// Will use this specific entity
    Sender(Entity),
    /// Will use all the clients for that server entity
    Server(Entity, NetworkTarget),
    /// Will assign to various ReplicationSenders to replicate to
    /// all peers in the NetworkTarget
    Target(NetworkTarget),
}

/// Insert this component to start replicating your entity.
///
/// - If sender is an Entity that has a ReplicationSender, we will replicate on that entity
/// - If the entity is None, we will try to find a unique ReplicationSender in the app
#[derive(Component, Clone, Default, Debug, PartialEq)]
pub struct Replicate {
    mode: ReplicationMode,
    pub(crate) senders: EntityIndexSet,
}

impl Replicate {
    pub fn new(mode: ReplicationMode) -> Self {
        Self {
            mode,
            senders: EntityIndexSet::default(),
        }
    }

    pub fn to_server() -> Self {
        Self {
            mode: ReplicationMode::SingleClient,
            senders: EntityIndexSet::default(),
        }
    }

    /// List of [`ReplicationSender`] entities that this entity is being replicated on
    pub fn senders(&self) -> impl Iterator<Item=Entity> {
        self.senders.iter().copied()
    }

    pub fn on_add(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we will use this world to access the ReplicationSender
            let world = unsafe { unsafe_world.world_mut() };
            // SAFETY: we will use this world only to access the Replicated entity, so there is no aliasing issue
            let mut replicate_entity_mut = unsafe { unsafe_world.world_mut().entity_mut(context.entity) };
            let mut replicate = replicate_entity_mut.get_mut::<Replicate>().unwrap();

            // enable split borrows
            let replicate = &mut *replicate;
            match &mut replicate.mode {
                ReplicationMode::SingleSender => {
                    let Ok((sender_entity, mut sender)) = world
                        .query::<(Entity, &mut ReplicationSender)>()
                        .single_mut(world)
                    else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    sender.add_replicated_entity(context.entity);
                    // SAFETY: the senders are guaranteed to be unique because OnAdd recreates the component from scratch
                    unsafe { replicate.senders.insert(sender_entity); }
                }
                ReplicationMode::SingleClient => {
                    let Ok((sender_entity, mut sender)) = world
                        .query_filtered::<(Entity, &mut ReplicationSender), With<Client>>()
                        .single_mut(world)
                    else {
                        error!("No Client found in the world");
                        return;
                    };
                    sender.add_replicated_entity(context.entity);
                    replicate.senders.insert(sender_entity);
                }
                ReplicationMode::SingleServer(target) => {
                    let unsafe_world = world.as_unsafe_world_cell();
                     // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let server_world = unsafe { unsafe_world.world_mut() };
                     let Some(server) = server_world.query::<&Server>().single(server_world) else {
                        error!("No Server found in the world");
                        return;
                    };
                    let world = unsafe { unsafe_world.world_mut() };
                    
                    server.targets(target).for_each(|client| {
                         let Ok(mut sender) = world
                            .query_filtered::<&mut ReplicationSender, With<ClientOf>>()
                             .get_mut(world, client)
                        else {
                            error!("No Client found in the world");
                            return;
                        };
                        sender.add_replicated_entity(context.entity);
                        replicate.senders.insert(client);
                    });
                }
                ReplicationMode::Sender(entity) => {
                    let Ok(mut sender) = world.query::<&mut ReplicationSender>().get_mut(world, *entity)
                    else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    sender.add_replicated_entity(context.entity);
                    replicate.senders.insert(*entity);
                }
                ReplicationMode::Server(server, target) => {
                     let unsafe_world = world.as_unsafe_world_cell();
                     // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                     let Some(server) = unsafe { unsafe_world.world() }.entity(*server).get::<Server>() else {
                        error!("No Server found in the world");
                        return;
                    };
                    let world = unsafe { unsafe_world.world_mut() };
                    server.targets(target).for_each(|client| {
                         let Ok(mut sender) = world
                             .query_filtered::<&mut ReplicationSender, With<ClientOf>>()
                             .get_mut(world, client)
                        else {
                            error!("No Client found in the world");
                            return;
                        };
                        sender.add_replicated_entity(context.entity);
                        replicate.senders.insert(client);
                    });
                }
                ReplicationMode::Target(_) => {
                    todo!("need a global mapping from remote_peer to corresponding replication_sender")
                }
            }
        });
    }
}




// TODO: component hook: immediately find the sender that we will replicate this on. Panic if no sender found

#[derive(Component)]
pub struct ReplicatedEntities{
    pub entities: Vec<Entity>,
}



pub(crate) fn replicate(
    // query &C + various replication components
    entity_query: Query<FilteredEntityRef>,
    mut manager_query: Query<(&mut ReplicationSender, &mut DeltaManager, &mut MessageManager, &LocalTimeline)>,
    component_registry: Res<ComponentRegistry>,
    system_ticks: SystemChangeTick,
    archetypes: &Archetypes,
    components: &Components,
    mut replicated_archetypes: Local<ReplicatedArchetypes<Replicate>>,
) {
    replicated_archetypes.update(archetypes, components, component_registry.as_ref());


    // TODO: iterate per entity first, and then per sender (using UniqueSlice)
    manager_query.par_iter_mut().for_each(|(mut sender, mut delta_manager, mut message_manager, timeline)| {
        // enable split borrows
        let mut sender = &mut *sender;

        // we iterate by index to avoid split borrow issues
        for i in 0..sender.replicated_entities.len() {
            let entity = sender.replicated_entities[i];
            // TODO: skip disabled entities?
            let Ok(entity_ref) = entity_query.get(entity) else {
                error!("Replicated Entity {:?} not found in entity_query", entity);
                return;
            };

            // get the value of the replication componentsk
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
                let Ok(root_entity_ref) = entity_query.get(replicate_like.0) else {
                    return;
                };
                if root_entity_ref.get::<Replicate>().is_none() {
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
                            .get_change_ticks::<Replicate>()
                            .or_else(|| root_entity_ref.get_change_ticks::<Replicate>())
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
                    // SAFETY: we know that the entity has the ReplicateOn component
                    // because the archetype is in replicated_archetypes
                    unsafe {
                        entity_ref
                            .get_change_ticks::<Replicate>()
                            .unwrap_unchecked()
                    },
                    false,
                )
            }};

            // the update will be 'insert' instead of update if the ReplicateOn component is new
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
                 buffer_entity_spawn(entity, group_id, priority, target_entity, sender);
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
                    timeline.tick(),
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
                    &mut message_manager.entity_mapper,
                    &mut sender,
                    &mut delta_manager,
                )
                .inspect_err(|e| {
                    error!(
                        "Error replicating component {:?} update for entity {:?}: {:?}",
                        kind, entity, e
                    )
                });
            }
        }
    });
}

/// Send entity spawn replication messages to server when the ReplicationTarget component is added
/// Also handles:
/// - handles TargetEntity if it's a Preexisting entity
/// - setting the priority
pub(crate) fn buffer_entity_spawn(
    entity: Entity,
    group_id: ReplicationGroupId,
    priority: f32,
    target_entity: Option<&TargetEntity>,
    sender: &mut ReplicationSender,
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
            .prepare_entity_spawn_reuse(entity, group_id, *remote_entity);
    } else {
        sender
            .prepare_entity_spawn(entity, group_id);
    }
    // also set the priority for the group when we spawn it
    sender
        .update_base_priority(group_id, priority);
}


/// Buffer entity despawn if an entity had [`Replicating`] and either:
/// - the [`Replicate`] component is removed
/// - is despawned
/// - [`ReplicateLike`] is removed
///
/// TODO: we do not currently handle the case where an entity is [`ReplicateLike`] another entity
///   and that root entity is despawned? Maybe [`ReplicateLike`] should be a relationship?
///
/// Note that if the entity does not [`Replicating`], we do not replicate the despawn
pub(crate) fn buffer_entity_despawn(
    // this covers both cases
    trigger: Trigger<OnRemove, (Replicate, ReplicateLike)>,
    root_query: Query<&ReplicateLike>,
    // only replicate the despawn event if the entity still has Replicating at the time of despawn
    // TODO: but how do we detect if both Replicating AND ReplicateToServer are removed at the same time?
    //  in which case we don't want to replicate the despawn.
    //  i.e. if a user wants to despawn an entity without replicating the despawn
    //  I guess we can provide a command that first removes Replicating, and then despawns the entity.
    entity_query: Query<(&ReplicationGroup, &Replicate), With<Replicating>>,
    mut query: Query<(&mut ReplicationSender, &mut MessageManager)>,
) {
    let mut entity = trigger.target();
    let root = root_query.get(entity).map_or(entity, |r| r.0);
    // TODO: use the child's ReplicationGroup if there is one that overrides the root's
    let Ok((group, replicate)) = entity_query.get(root) else {
        return
    };
    trace!(?entity, "Buffering entity despawn");
    query
        .par_iter_many_unique_mut(replicate.senders.as_slice())
        .for_each(|(mut sender, mut manager)| {
        // convert the entity to a network entity (possibly mapped)
        entity = manager
            .entity_mapper
            .to_remote(entity);
        sender.prepare_entity_despawn(entity, group.group_id(Some(entity)));
    });
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
    entity_map: &mut RemoteEntityMap,
    sender: &mut ReplicationSender,
    delta: &mut DeltaManager,
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
        // NOTE: we have to apply the entity mapping here because we are sending the message directly to the Transport
        //  instead of relying on the MessageManagers' remote_entity_map. This is because using the MessageManager
        //  wouldn't give us back a MessageId.
        entity = entity_map
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
                        &mut entity_map
                            .local_to_remote,
                    )?
                }
            } else {
                component_registry.erased_serialize(
                    component_data,
                    writer,
                    component_kind,
                    &mut entity_map
                        .local_to_remote,
                )?;
            };
            let raw_data = writer.split();
            sender.prepare_component_insert(entity, group_id, raw_data);
        } else {
            trace!(?entity, "send update");
            let send_tick = sender
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
                    sender.prepare_delta_component_update(
                        entity,
                        group_id,
                        component_kind,
                        component_data,
                        component_registry,
                        delta,
                        current_tick,
                        entity_map,
                    )?;
                } else {
                    component_registry.erased_serialize(
                        component_data,
                        writer,
                        component_kind,
                        &mut entity_map.local_to_remote
                    )?;
                    let raw_data = writer.split();
                    sender.prepare_component_update(entity, group_id, raw_data);
                }
            }
        }
    }
    Ok(())
}

/// Send component remove message when a component gets removed
// TODO: use a common observer for all removed components
pub(crate) fn buffer_component_removed<C: Component>(
    trigger: Trigger<OnRemove, C>,
    registry: Res<ComponentRegistry>,
    root_query: Query<&ReplicateLike>,
    // only remove the component for entities that are being actively replicated
    query: Query<
        (&ReplicationGroup, &Replicate, Option<&DisabledComponents>),
        (With<Replicating>, With<Replicate>),
    >,
    mut manager_query: Query<(&mut ReplicationSender, &mut MessageManager)>,
) {
    let mut entity = trigger.target();
    let root = root_query.get(entity).map_or(entity, |r| r.0);
    // TODO: be able to override the root components with those from the child
    let Ok((group, replicate_on, disabled_components)) = query.get(root) else {
        return
    };

    manager_query
        .par_iter_many_unique_mut(replicate_on.senders.as_slice())
        .for_each(|(mut sender, mut manager)| {
         // convert the entity to a network entity (possibly mapped)
        entity = manager
            .entity_mapper
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
        sender.prepare_component_remove(entity, group_id, kind);
    });
}

pub(crate) fn register_replicate_component_send<C: Component>(app: &mut App) {
    // TODO: what if we remove and add within one replication_interval?
    app.add_observer(buffer_component_removed::<C>);
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
            .spawn(ChildOf {
                parent: client_entity,
            })
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
            .spawn(ChildOf {
                parent: client_entity,
            })
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
            .spawn(ChildOf {
                parent: client_entity,
            })
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
            .spawn(ChildOf {
                parent: client_entity,
            })
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
                ChildOf {
                    parent: client_entity,
                },
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
