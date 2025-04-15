//! Client replication plugins
use bevy::prelude::*;
use core::time::Duration;

use super::*;
use bevy::ecs::archetype::Archetypes;
use bevy::ecs::component::{ComponentId, ComponentTicks, Components, HookContext};
use bevy::ecs::entity::{EntityIndexSet, UniqueEntitySlice, UniqueEntityVec};
use bevy::ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
use bevy::ecs::world::{DeferredWorld, FilteredEntityRef};
use bevy::platform_support::collections::HashSet;
use bevy::ptr::Ptr;

use crate::archetypes::{ReplicatedArchetypes, ReplicatedComponent};
use crate::authority::HasAuthority;
use crate::components::{
    Replicating, ReplicationGroup,
    ReplicationGroupId,
};
use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
use crate::prelude::{Cached, ComponentReplicationOverride, ComponentReplicationOverrides};
use crate::receive::ReplicationReceiver;
use crate::registry::registry::ComponentRegistry;
use crate::registry::ComponentKind;
use crate::send::ReplicationSender;
use crate::visibility::immediate::NetworkVisibility;
#[cfg(feature = "client")]
use lightyear_connection::client::Client;
#[cfg(feature = "server")]
use lightyear_connection::client_of::{ClientOf, Server};
use lightyear_connection::prelude::NetworkTarget;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{LocalTimeline, NetworkTimeline};
use lightyear_messages::MessageManager;
use lightyear_serde::entity_map::RemoteEntityMap;
use lightyear_transport::prelude::Transport;
use tracing::{info, trace};

#[derive(Clone, Default, Debug, PartialEq, Reflect)]
pub enum ReplicationMode {
    /// Will try to find a single ReplicationSender entity in the world
    #[default]
    SingleSender,
    #[cfg(feature = "client")]
    /// Will try to find a single Client entity in the world
    SingleClient,
    #[cfg(feature = "server")]
    /// Will try to find a single Server entity in the world
    SingleServer(NetworkTarget),
    /// Will use this specific entity
    Sender(Entity),
    #[cfg(feature = "server")]
    /// Will use all the clients for that server entity
    Server(Entity, NetworkTarget),
    /// Will assign to various ReplicationSenders to replicate to
    /// all peers in the NetworkTarget
    Target(NetworkTarget),
    Manual(Vec<Entity>),
}

/// Insert this component to start replicating your entity.
///
/// - If sender is an Entity that has a ReplicationSender, we will replicate on that entity
/// - If the entity is None, we will try to find a unique ReplicationSender in the app
#[derive(Component, Clone, Default, Debug, PartialEq)]
#[require(HasAuthority)]
#[require(Replicating)]
#[require(ReplicationGroup)]
#[component(on_insert = Replicate::on_insert)]
#[component(on_replace = Replicate::on_replace)]
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

    #[cfg(feature = "client")]
    pub fn to_server() -> Self {
        Self {
            mode: ReplicationMode::SingleClient,
            senders: EntityIndexSet::default(),
        }
    }

    pub fn manual(senders: Vec<Entity>) -> Self {
        Self {
            mode: ReplicationMode::Manual(senders),
            senders: EntityIndexSet::default(),
        }
    }

    /// List of [`ReplicationSender`] entities that this entity is being replicated on
    pub fn senders(&self) -> impl Iterator<Item = Entity> {
        self.senders.iter().copied()
    }

    pub fn on_insert(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we will use this world to access the ReplicationSender
            let world = unsafe { unsafe_world.world_mut() };
            // SAFETY: we will use this world only to access the Replicated entity, so there is no aliasing issue
            let mut replicate_entity_mut =
                unsafe { unsafe_world.world_mut().entity_mut(context.entity) };

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
                    unsafe {
                        replicate.senders.insert(sender_entity);
                    }
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    let Ok((sender_entity, mut sender)) = world
                        .query_filtered::<(Entity, &mut ReplicationSender), With<Client>>()
                        .single_mut(world)
                    else {
                        error!("No Client found in the world");
                        return;
                    };
                    debug!(
                        "Adding replicated entity {} to sender {}",
                        context.entity, sender_entity
                    );
                    sender.add_replicated_entity(context.entity);
                    replicate.senders.insert(sender_entity);
                }
                #[cfg(feature = "server")]
                ReplicationMode::SingleServer(target) => {
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let server_world = unsafe { unsafe_world.world_mut() };
                    let Ok(server) = server_world.query::<&Server>().single(server_world) else {
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
                    let Ok(mut sender) = world
                        .query::<&mut ReplicationSender>()
                        .get_mut(world, *entity)
                    else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    sender.add_replicated_entity(context.entity);
                    replicate.senders.insert(*entity);
                }
                #[cfg(feature = "server")]
                ReplicationMode::Server(server, target) => {
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let Some(server) = unsafe { unsafe_world.world() }
                        .entity(*server)
                        .get::<Server>()
                    else {
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
                    todo!(
                        "need a global mapping from remote_peer to corresponding replication_sender"
                    )
                },
                ReplicationMode::Manual(sender_entities) => {
                    for sender_entity in sender_entities.iter() {
                        let Ok(mut sender) = world
                            .query::<&mut ReplicationSender>()
                            .get_mut(world, *sender_entity)
                        else {
                            error!("No ReplicationSender found in the world");
                            return;
                        };
                        sender.add_replicated_entity(context.entity);
                        replicate.senders.insert(*sender_entity);
                    }
                }
            }

        });
    }

    pub fn on_replace(mut world: DeferredWorld, context: HookContext) {
        // TODO: maybe we can just use the CachedReplicate?
        // i.e. if you remove 2 clients from Replicate, than in PreBuffer, we will do the diff
        // and remove those clients from sender.replicated_entities and send the despawn

        let mut replicate = world.get_mut::<Replicate>(context.entity).unwrap();
        core::mem::take(&mut replicate.senders)
            .iter()
            .for_each(|sender_entity| {
                if let Some(mut sender) = world.get_mut::<ReplicationSender>(*sender_entity) {
                    sender.replicated_entities.swap_remove(&context.entity);
                }
            });
    }
}

// TODO: component hook: immediately find the sender that we will replicate this on. Panic if no sender found

#[derive(Component, Debug)]
pub struct CachedReplicate {
    senders: EntityIndexSet,
}



/// Keep a cached version of the [`Replicate`] component so that when it gets updated
/// we can compute a diff from the previous value.
///
/// This needs to run after we compute the diff, so after the `replicate` system runs
pub(crate) fn update_cached_replicate_post_buffer(
    mut commands: Commands,
    mut query: Query<(Entity, &Replicate, Option<&mut CachedReplicate>), Changed<Replicate>>,
) {
    for (entity, replicate, cached) in query.iter_mut() {
        if let Some(mut cached) = cached {
            cached.senders = replicate.senders.clone();
        } else {
            commands.entity(entity).insert(CachedReplicate {
                senders: replicate.senders.clone(),
            });
        }
    }
}

pub(crate) fn replicate(
    // query &C + various replication components
    entity_query: Query<FilteredEntityRef>,
    // TODO: should we put the DeltaManager in the same component?
    mut manager_query: Query<(
        Entity,
        &mut ReplicationSender,
        &mut DeltaManager,
        &mut MessageManager,
        &LocalTimeline,
    )>,
    component_registry: Res<ComponentRegistry>,
    system_ticks: SystemChangeTick,
    archetypes: &Archetypes,
    components: &Components,
    mut replicated_archetypes: Local<ReplicatedArchetypes>,
) {
    replicated_archetypes.update(archetypes, components, component_registry.as_ref());

    // TODO: iterate per entity first, and then per sender (using UniqueSlice)
    manager_query.par_iter_mut().for_each(
        |(sender_entity, mut sender, mut delta_manager, mut message_manager, timeline)| {
            let tick = timeline.tick();

            // enable split borrows
            let mut sender = &mut *sender;

            // we iterate by index to avoid split borrow issues
            for i in 0..sender.replicated_entities.len() {
                let entity = sender.replicated_entities[i];
                let Ok(root_entity_ref) = entity_query.get(entity) else {
                    error!("Replicated Entity {:?} not found in entity_query", entity);
                    return;
                };
                replicate_entity(
                    entity,
                    tick,
                    &root_entity_ref,
                    None,
                    &system_ticks,
                    &mut message_manager.entity_mapper,
                    sender,
                    sender_entity,
                    component_registry.as_ref(),
                    &replicated_archetypes,
                    &mut delta_manager,
                );
                if let Some(children) = root_entity_ref.get::<ReplicateLikeChildren>() {
                    for child in children.collection() {
                        let child_entity_ref = entity_query.get(*child).unwrap();
                        replicate_entity(
                            *child,
                            tick,
                            &root_entity_ref,
                            Some((child_entity_ref, entity)),
                            &system_ticks,
                            &mut message_manager.entity_mapper,
                            sender,
                            sender_entity,
                            component_registry.as_ref(),
                            &replicated_archetypes,
                            &mut delta_manager,
                        );
                    }
                }
            }

            // TODO: maybe this should be in a separate system in AfterBuffer?
            // cleanup after buffer
            sender.tick_cleanup(tick);
        },
    );
}

pub(crate) fn replicate_entity(
    entity: Entity,
    tick: Tick,
    entity_ref: &FilteredEntityRef,
    child_entity_ref: Option<(FilteredEntityRef, Entity)>,
    system_ticks: &SystemChangeTick,
    entity_mapper: &mut RemoteEntityMap,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
    component_registry: &ComponentRegistry,
    replicated_archetypes: &ReplicatedArchetypes,
    delta_manager: &mut DeltaManager,
) {
    // get the value of the replication components
    let (
        group_id,
        priority,
        group_ready,
        replicate,
        cached_replicate,
        network_visibility,
        entity_ref,
        is_replicate_like_added,
    ) = match &child_entity_ref {
        Some((child_entity_ref, root)) => {
            let (group_id, priority, group_ready) =
                child_entity_ref.get::<ReplicationGroup>().map_or_else(
                    // if ReplicationGroup is not present, we use the parent entity
                    || {
                        entity_ref
                            .get::<ReplicationGroup>()
                            .map(|g| {
                                (
                                    g.group_id(Some(*root)),
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
                // We use the root entity's Replicate/CachedReplicate component
                // SAFETY: we know that the root entity has the Replicate component
                entity_ref.get_ref::<Replicate>().unwrap(),
                entity_ref.get::<CachedReplicate>(),
                child_entity_ref
                    .get::<NetworkVisibility>()
                    .or_else(|| entity_ref.get::<NetworkVisibility>()),
                child_entity_ref,
                unsafe {
                    child_entity_ref
                        .get_change_ticks::<ReplicateLike>()
                        .unwrap_unchecked()
                        .is_changed(system_ticks.last_run(), system_ticks.this_run())
                },
            )
        }
        _ => {
            let (group_id, priority, group_ready) = entity_ref
                .get::<ReplicationGroup>()
                .map(|g| (g.group_id(Some(entity)), g.priority(), g.should_send))
                .unwrap();
            (
                group_id,
                priority,
                group_ready,
                entity_ref.get_ref::<Replicate>().unwrap(),
                entity_ref.get::<CachedReplicate>(),
                entity_ref.get::<NetworkVisibility>(),
                entity_ref,
                false,
            )
        }
    };
    let replicated_components = replicated_archetypes
        .archetypes
        .get(&entity_ref.archetype().id())
        .unwrap();

    // the update will be 'insert' instead of update if the ReplicateOn component is new
    // or the HasAuthority component is new. That's because the remote cannot receive update
    // without receiving an action first (to populate the latest_tick on the replication-receiver)

    // TODO: do the entity mapping here!

    // b. add entity despawns from Visibility lost
    replicate_entity_despawn(
        entity,
        group_id,
        entity_mapper,
        network_visibility,
        sender,
        sender_entity,
    );

    // c. add entity spawns for Replicate changing
    replicate_entity_spawn(
        entity,
        group_id,
        priority,
        &replicate,
        cached_replicate,
        network_visibility,
        sender,
        sender_entity,
        is_replicate_like_added,
    );

    // If the group is not set to send, skip this entity
    if !group_ready {
        return;
    }

    // d. all components that were added or changed and that are not disabled

    // NOTE: we pre-cache the list of components for each archetype to not iterate through
    //  all replicated components every time
    for ReplicatedComponent { id, kind, has_overrides } in replicated_components {
        let comp = component_registry.serialize_fns_map.get(kind).unwrap().type_name;


        let mut replication_metadata = component_registry.replication_map.get(kind).unwrap();
        let mut disable = replication_metadata.config.disable;
        let mut replicate_once = replication_metadata.config.replicate_once;
        let mut delta_compression = replication_metadata.config.delta_compression;
        if *has_overrides {
            // TODO: get ComponentReplicationOverrides using root entity
            // SAFETY: we know that all overrides have the same shape
            if let Some(overrides) = unsafe { entity_ref.get_by_id(replication_metadata.overrides_component_id).unwrap().deref::<ComponentReplicationOverrides<Replicate>>() }.get_overrides(sender_entity) {
                if disable && overrides.enable {
                    disable = false;
                }
                if !disable && overrides.disable {
                    disable = true;
                }
                if replicate_once && overrides.replicate_always {
                    replicate_once = false;
                }
                if !replicate_once && overrides.replicate_once {
                    replicate_once = true;
                }
            }
        }
        if disable {
            continue;
        }
        let Some(data) = entity_ref.get_by_id(*id) else {
            // component not present on entity, skip
            return;
        };
        let component_ticks = entity_ref.get_change_ticks_by_id(*id).unwrap();
        let _ = replicate_component_update(
            tick,
            component_registry,
            entity,
            *kind,
            data,
            component_ticks,
            &replicate,
            group_id,
            delta_compression,
            replicate_once,
            system_ticks,
            entity_mapper,
            sender,
            delta_manager,
        )
        .inspect_err(|e| {
            error!(
                "Error replicating component {:?} update for entity {:?}: {:?}",
                kind, entity, e
            )
        });
    }
}


/// Send entity despawn if Replicate was updated and the entity should not be replicated to this sender anymore
/// This cannot be part of `replicate` because replicate iterates through the sender's replicated_entities and
/// the entity was removed from the sender's replicated_entities list
pub(crate) fn buffer_entity_despawn_replicate_updated(
    query: Query<(Entity, &ReplicationGroup, &Replicate, &CachedReplicate)>,
    mut senders: Query<&mut ReplicationSender>
) {
    query.iter().for_each(|(entity, group, replicate, cached_replicate)| {
        let group_id = group.group_id(Some(entity));
        cached_replicate.senders.difference(&replicate.senders).for_each(|sender_entity| {
            if let Ok(mut sender) = senders.get_mut(*sender_entity) {
                trace!(?entity, ?sender_entity, ?replicate, ?cached_replicate, "Sending Despawn because replicate changed");
                sender.prepare_entity_despawn(entity, group_id);
            }
        })
    });
}

/// Send entity despawn is:
/// 1) the client lost visibility of the entity
pub(crate) fn replicate_entity_despawn(
    entity: Entity,
    group_id: ReplicationGroupId,
    entity_map: &mut RemoteEntityMap,
    visibility: Option<&NetworkVisibility>,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
) {
    if visibility.is_some_and(|v| v.lost.contains(&sender_entity)) {
        let entity = entity_map.to_remote(entity);
        sender.prepare_entity_despawn(entity, group_id);
    }
}

/// Send entity spawn if either of:
/// 1) Replicate was added/updated and the sender was not in the previous Replicate's target
/// 2) NetworkVisibility is gained for this sender
/// 3) ReplicateLike was updated
// TODO: 3) is not perfect, ReplicateLike could be changing from one entity to another, and in that case we don't want
//  to send Spawn again
pub(crate) fn replicate_entity_spawn(
    entity: Entity,
    group_id: ReplicationGroupId,
    priority: f32,
    replicate: &Ref<Replicate>,
    cached_replicate: Option<&CachedReplicate>,
    network_visibility: Option<&NetworkVisibility>,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
    is_replicate_like_added: bool,
) {
    // 1. replicate was added/updated and the sender was not in the previous Replicate's target
    let replicate_updated = replicate.is_changed() && cached_replicate.is_none_or(|cached| !cached.senders.contains(&sender_entity)) && network_visibility.is_none_or(|vis| vis.is_visible(sender_entity));
    // 2. replicate was not updated but NetworkVisibility is gained for this sender
    let network_visibility_updated = network_visibility.is_some_and(|vis| vis.gained.contains(&sender_entity));
    if replicate_updated || network_visibility_updated || is_replicate_like_added {
        trace!(?entity, ?sender_entity, ?replicate, ?cached_replicate, ?replicate_updated, ?network_visibility_updated, "Sending Spawn");
        sender.prepare_entity_spawn(entity, group_id, priority);
    }
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
pub(crate) fn buffer_entity_despawn_replicate_remove(
    // this covers both cases
    trigger: Trigger<OnRemove, (Replicate, ReplicateLike)>,
    root_query: Query<&ReplicateLike>,
    // only replicate the despawn event if the entity still has Replicating at the time of despawn
    // TODO: but how do we detect if both Replicating AND ReplicateToServer are removed at the same time?
    //  in which case we don't want to replicate the despawn.
    //  i.e. if a user wants to despawn an entity without replicating the despawn
    //  I guess we can provide a command that first removes Replicating, and then despawns the entity.
    entity_query: Query<(&ReplicationGroup, &CachedReplicate, Option<&NetworkVisibility>), With<Replicating>>,
    mut query: Query<(Entity, &mut ReplicationSender, &mut MessageManager)>,
) {
    let mut entity = trigger.target();
    let root = root_query.get(entity).map_or(entity, |r| r.root);
    // TODO: use the child's ReplicationGroup if there is one that overrides the root's
    let Ok((group, cached_replicate, network_visibility)) = entity_query.get(root) else {
        return;
    };
    trace!(?entity, ?cached_replicate, "Buffering entity despawn");
    // TODO: if ReplicateLike is removed, we need to use the root entity's Replicate
    //  if Replicate is removed, we need to use the CachedReplicate (since Replicate is updated immediately via hook)
    //  for the root_entity and its ReplicateLike children

    // If the entity has NetworkVisibility, we only send the Despawn to the senders that have visibility
    // of this entity. Otherwise we send it to all senders that have the entity in their replicated_entities

    query
        .par_iter_many_unique_mut(cached_replicate.senders.as_slice())
        .for_each(|(sender_entity, mut sender, mut manager)| {
            if network_visibility.is_some_and(|v| !v.is_visible(sender_entity)) {
                trace!(?entity, ?sender_entity, "Not sending despawn because the sender didn't have visibility of the entity");
                return;
            }
            // convert the entity to a network entity (possibly mapped)
            let entity = manager.entity_mapper.to_remote(entity);
            sender.prepare_entity_despawn(entity, group.group_id(Some(entity)));
            trace!("prepareing despawn to sender");
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
    replicate: &Ref<Replicate>,
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
    if component_ticks.is_added(system_ticks.last_run(), system_ticks.this_run()) || replicate.is_changed() {
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
        entity = entity_map.to_remote(entity);

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
                        &mut entity_map.local_to_remote,
                    )?
                }
            } else {
                component_registry.erased_serialize(
                    component_data,
                    writer,
                    component_kind,
                    &mut entity_map.local_to_remote,
                )?;
            };
            let raw_data = writer.split();
            sender.prepare_component_insert(entity, group_id, raw_data);
        } else {
            trace!(?entity, "send update");
            let send_tick = sender.group_channels.entry(group_id).or_default().send_tick;

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
                        &mut entity_map.local_to_remote,
                    )?;
                    let raw_data = writer.split();
                    sender.prepare_component_update(entity, group_id, raw_data);
                }
            }
        }
    }
    Ok(())
}

// Removals for all replicated components
// - check if the entity is in the sender's replication components

/// Send component remove message when a component gets removed
// TODO: use a common observer for all removed components
// TODO: you could have a case where you remove a component C, and then afterwards
//   modify the replication target, but we still send messages to the old components.
//   Maybe we should just add the components to a buffer?
pub(crate) fn buffer_component_removed(
    trigger: Trigger<OnRemove>,
    // Query<&C, Or<With<ReplicateLike>, (With<Replicate>, With<ReplicationGroup>)>>
    query: Query<FilteredEntityRef>,
    registry: Res<ComponentRegistry>,
    root_query: Query<&ReplicateLike>,
    mut manager_query: Query<(Entity, &mut ReplicationSender, &mut MessageManager)>,
) {
    let entity = trigger.target();
    let root = root_query.get(entity).map_or(entity, |r| r.root);
    let Ok(entity_ref) = query.get(root) else {
        return;
    };
    let Some(group) = entity_ref.get::<ReplicationGroup>() else {
        return;
    };
    let group_id = group.group_id(Some(root));
    let Some(replicate) = entity_ref.get::<Replicate>() else {
        return;
    };
    manager_query
        .par_iter_many_unique_mut(replicate.senders.as_slice())
        .for_each(|(sender_entity, mut sender, mut manager)| {
            // convert the entity to a network entity (possibly mapped)
            let entity = manager.entity_mapper.to_remote(entity);
            for component_id in trigger.components() {
                // check if the component is disabled
                let kind = registry.component_id_to_kind.get(component_id).unwrap();
                let metadata = registry.replication_map.get(kind).unwrap();
                let mut disable = metadata.config.disable;
                if let Some(overrides) = entity_ref.get_by_id(metadata.overrides_component_id).and_then(|o| unsafe { o.deref::<ComponentReplicationOverrides<Replicate>>() }.get_overrides(sender_entity)) {
                    if disable && overrides.enable {
                        disable = false;
                    }
                    if !disable && overrides.disable {
                        disable = true;
                    }
                }
                if disable {
                    continue;
                }
                trace!(?entity, ?kind, "Sending RemoveComponent");
                let net_id = *registry.kind_map.net_id(kind).unwrap();
                sender.prepare_component_remove(entity, group_id, net_id);
            }
        });
}