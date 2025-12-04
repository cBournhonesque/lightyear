use crate::components::ComponentReplicationOverrides;
use crate::control::{Controlled, ControlledBy};
use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
use crate::prelude::{PerSenderReplicationState, ReplicationState};
use crate::prespawn::PreSpawned;
use crate::registry::ComponentKind;
use crate::registry::registry::ComponentRegistry;
use crate::send::archetypes::{ReplicatedArchetypes, ReplicatedComponent};
use crate::send::components::{Replicate, Replicating, ReplicationGroup, ReplicationGroupId};
use crate::send::plugin::ReplicableRootEntities;
use crate::send::sender::ReplicationSender;
use crate::visibility::immediate::VisibilityState;
use bevy_ecs::component::Components;
use bevy_ecs::prelude::*;
use bevy_ecs::{
    archetype::Archetypes, component::ComponentTicks, relationship::RelationshipTarget,
    system::SystemChangeTick, world::FilteredEntityRef,
};
use bevy_ptr::Ptr;
use lightyear_connection::client::Connected;
use lightyear_connection::host::HostClient;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{LocalTimeline, NetworkTimeline};
use lightyear_link::prelude::Server;
use lightyear_link::server::LinkOf;
use lightyear_messages::MessageManager;
use lightyear_serde::entity_map::RemoteEntityMap;
#[cfg(feature = "metrics")]
use lightyear_utils::metrics::DormantTimerGauge;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, error, info, info_span, trace, trace_span, warn};

pub(crate) fn replicate(
    // query &C + various replication components
    // we know that we always query Replicate from the parent
    mut query: ParamSet<(Query<FilteredEntityRef>, Query<&mut ReplicationState>)>,
    mut manager_query: Query<
        (
            Entity,
            &mut ReplicationSender,
            &mut MessageManager,
            &LocalTimeline,
            Option<&DeltaManager>,
            Option<&LinkOf>,
        ),
        // On the Host-Client there is no replication messages to send since the entities
        // from the sender are in the same world!
        (With<Connected>, Without<HostClient>),
    >,
    delta_query: Query<&DeltaManager, With<Server>>,
    replicable_entities: Res<ReplicableRootEntities>,
    component_registry: Res<ComponentRegistry>,
    system_ticks: SystemChangeTick,
    archetypes: &Archetypes,
    components: &Components,
    mut replicated_archetypes: Local<ReplicatedArchetypes>,
) {
    #[cfg(feature = "metrics")]
    let _timer = DormantTimerGauge::new("replication/buffer");

    replicated_archetypes.update(archetypes, components, component_registry.as_ref());

    // NOTE: this system doesn't handle delta compression, because we need to store a shared component history
    // for delta components, which is not possible when we start iterating through the senders first.
    // Maybe the easiest would be to simply store the component history for every tick where the sender is ready to send?
    // (this assumes that the senders are all sending at the same tick). Otherwise we store the component history for all
    // past ticks where the component changes.
    let p0 = query.p0();
    manager_query.par_iter_mut().for_each(
        |(sender_entity, mut sender, mut message_manager, timeline, delta_manager, link_of)| {

            let _span = trace_span!("replicate", sender = ?sender_entity).entered();

            // enable split borrows
            let sender = &mut *sender;
            if !sender.send_timer.is_finished() {
                return;
            }
            #[cfg(feature = "metrics")]
            _timer.activate();

            // delta: either the delta manager is present on the sender directly (Client)
            // or the delta is on the server
            let delta = delta_manager
                .or_else(|| {
                    link_of.and_then(|l| delta_query.get(l.server).ok())
                });
            let tick = timeline.tick();

            // update the change ticks
            sender.last_run = sender.this_run;
            sender.this_run = system_ticks.this_run();

            // TODO: maybe this should be in a separate system in AfterBuffer?
            // run any possible tick cleanup
            sender.tick_cleanup(tick);

            trace!(
                this_run = ?sender.this_run,
                last_run = ?sender.last_run,
                "Starting buffer replication for sender {sender_entity:?}");
            replicable_entities.entities.iter().for_each(|&entity| {
                let Ok(root_entity_ref) = p0.get(entity) else {
                    trace!("Replicated Entity {:?} not found in entity_query. This could be because Replicating is not present on the entity", entity);
                    return;
                };
                let _root_span = trace_span!("root", ?entity).entered();
                replicate_entity(
                    entity,
                    tick,
                    &root_entity_ref,
                    None,
                    &mut message_manager.entity_mapper,
                    sender,
                    sender_entity,
                    component_registry.as_ref(),
                    &replicated_archetypes,
                    delta,
                );
                if let Some(children) = root_entity_ref.get::<ReplicateLikeChildren>() {
                    for child in children.collection() {
                        let _child_span = trace_span!("child", ?child).entered();
                        let child_entity_ref = p0.get(*child).unwrap();
                        replicate_entity(
                            *child,
                            tick,
                            &root_entity_ref,
                            Some(&(child_entity_ref, entity)),
                            &mut message_manager.entity_mapper,
                            sender,
                            sender_entity,
                            component_registry.as_ref(),
                            &replicated_archetypes,
                            delta,
                        );
                    }
                }
            });

            // Drain all entities that should be despawned because Replicate changed and they are not in the new
            // Replicate's senders list anymore
            sender.prepare_entity_despawns();
        },
    );

    // update the metadata that tracks on which sender the entity was sent
    manager_query
        .iter_mut()
        .for_each(|(sender_entity, mut sender, _, _, _, _)| {
            sender.new_spawns.drain(..).for_each(|e| {
                if let Ok(mut state) = query.p1().get_mut(e)
                    && let Some(s) = state.per_sender_state.get_mut(&sender_entity)
                {
                    s.spawned = true;
                }
            })
        })
}

#[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
pub(crate) fn replicate_entity(
    entity: Entity,
    tick: Tick,
    root_entity_ref: &FilteredEntityRef,
    child_entity_ref: Option<&(FilteredEntityRef, Entity)>,
    entity_mapper: &mut RemoteEntityMap,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
    component_registry: &ComponentRegistry,
    replicated_archetypes: &ReplicatedArchetypes,
    delta: Option<&DeltaManager>,
) {
    // get the value of the replication components
    let (
        group_id,
        priority,
        group_ready,
        replication_state,
        // TODO: fetch owned_by only if needed
        owned_by,
        entity_ref,
        is_replicate_like_added,
    ) = match child_entity_ref {
        Some((child_entity_ref, root)) => {
            let (group_id, priority, group_ready) =
                child_entity_ref.get::<ReplicationGroup>().map_or_else(
                    // if ReplicationGroup is not present, we use the root entity
                    || {
                        root_entity_ref
                            .get::<ReplicationGroup>()
                            .map(|g| (g.group_id(Some(*root)), g.priority(), g.should_send))
                            .unwrap()
                    },
                    // we use the entity itself if ReplicationGroup is present
                    |g| (g.group_id(Some(entity)), g.priority(), g.should_send),
                );
            (
                group_id,
                priority,
                group_ready,
                child_entity_ref
                    .get::<ReplicationState>()
                    .unwrap_or_else(|| root_entity_ref.get::<ReplicationState>().unwrap()),
                child_entity_ref
                    .get::<ControlledBy>()
                    .or_else(|| root_entity_ref.get::<ControlledBy>()),
                child_entity_ref,
                unsafe {
                    sender.is_updated(
                        child_entity_ref
                            .get_change_ticks::<ReplicateLike>()
                            .unwrap_unchecked()
                            .changed,
                    )
                },
            )
        }
        _ => {
            let (group_id, priority, group_ready) = root_entity_ref
                .get::<ReplicationGroup>()
                .map(|g| (g.group_id(Some(entity)), g.priority(), g.should_send))
                .unwrap();
            (
                group_id,
                priority,
                group_ready,
                root_entity_ref.get::<ReplicationState>().unwrap(),
                root_entity_ref.get::<ControlledBy>(),
                root_entity_ref,
                false,
            )
        }
    };
    let Some(state) = replication_state.per_sender_state.get(&sender_entity) else {
        return;
    };
    if state.authority.is_none_or(|a| !a) {
        return;
    }

    // we use the entity's PreSpawned component (we cannot re-use the root's)
    let prespawned = entity_ref.get::<PreSpawned>();
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
        &state.visibility,
        sender,
        sender_entity,
    );

    if !state.visibility.is_visible() {
        return;
    }

    // c. add entity spawns for Replicate changing
    let spawn = replicate_entity_spawn(
        entity,
        group_id,
        priority,
        state,
        prespawned,
        owned_by,
        entity_mapper,
        component_registry,
        sender,
        sender_entity,
        is_replicate_like_added,
    );

    // If the group is not set to send, skip this entity
    if !group_ready {
        return;
    }

    // d. all components that were added or changed and that are not disabled

    // convert the entity to a network entity (possibly mapped)
    // NOTE: we have to apply the entity mapping here because we are sending the message directly to the Transport
    //  instead of relying on the MessageManagers' remote_entity_map. This is because using the MessageManager
    //  wouldn't give us back a MessageId.
    let mapped_entity = entity_mapper.to_remote(entity);

    // NOTE: we pre-cache the list of components for each archetype to not iterate through
    //  all replicated components every time
    for ReplicatedComponent {
        id,
        kind,
        has_overrides,
    } in replicated_components
    {
        let replication_metadata = component_registry
            .component_metadata_map
            .get(kind)
            .unwrap()
            .replication
            .as_ref()
            .unwrap();
        let mut disable = replication_metadata.config.disable;
        let mut replicate_once = replication_metadata.config.replicate_once;
        let delta_compression = replication_metadata.config.delta_compression;
        if *has_overrides {
            // TODO: get ComponentReplicationOverrides using root entity
            // SAFETY: we know that all overrides have the same shape
            if let Some(overrides) = unsafe {
                entity_ref
                    .get_by_id(replication_metadata.overrides_component_id)
                    .unwrap()
                    .deref::<ComponentReplicationOverrides<Replicate>>()
            }
            .get_overrides(sender_entity)
            {
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
            mapped_entity,
            *kind,
            data,
            component_ticks,
            group_id,
            delta_compression,
            replicate_once,
            entity_mapper,
            sender,
            delta,
            spawn,
        )
        .inspect_err(|e| {
            error!(
                "Error replicating component {:?} update for entity {:?}: {:?}",
                kind, entity, e
            )
        });
    }
}

/// Send entity despawn is:
/// 1) the client lost visibility of the entity
pub(crate) fn replicate_entity_despawn(
    entity: Entity,
    group_id: ReplicationGroupId,
    entity_map: &mut RemoteEntityMap,
    visibility: &VisibilityState,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
) {
    if matches!(visibility, &VisibilityState::Lost) {
        debug!(
            ?entity,
            ?sender_entity,
            "Replicate entity despawn because visibility lost"
        );
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
#[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
pub(crate) fn replicate_entity_spawn(
    entity: Entity,
    group_id: ReplicationGroupId,
    priority: f32,
    state: &PerSenderReplicationState,
    #[allow(unused_mut)] mut prespawned: Option<&PreSpawned>,
    controlled_by: Option<&ControlledBy>,
    entity_map: &mut RemoteEntityMap,
    component_registry: &ComponentRegistry,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
    is_replicate_like_added: bool,
) -> bool {
    // if the local entity is already mapped (for example because of authority transfer or PrePrediction), then
    // there is no need to send a Spawn
    if entity_map.get_remote(entity).is_some() {
        trace!(
            ?entity,
            ?group_id,
            "Not sending Spawn because entity is already mapped"
        );
        return false;
    }

    let visible = state.visibility.is_visible();
    // 1. the sender got added to the list of senders for this entity's Replicate but we haven't spawned the entity
    //    yet for this sender
    //    Checking if `Replicate` is changed is not enough, because we don't to re-send Spawn for entities we have
    //    already replicated.
    let should_spawn = !state.spawned;
    // 2. replicate was not updated but NetworkVisibility is gained for this sender
    let network_visibility_gained = state.visibility == VisibilityState::Gained;
    // 3. replicate-like was added and the the entity is visible for this sender
    let spawn = (visible && (should_spawn || is_replicate_like_added)) || network_visibility_gained;
    if spawn {
        // mark that this entity has been spawned to this sender!
        sender.new_spawns.push(entity);
        debug!(
            ?entity,
            ?group_id,
            ?visible,
            ?state,
            ?should_spawn,
            ?network_visibility_gained,
            ?is_replicate_like_added,
            "Sending Spawn"
        );
        if state.interpolated {
            // if the entity is interpolated, we don't want to Prespawn it
            prespawned = None;
        }
        sender.prepare_entity_spawn(
            entity,
            group_id,
            priority,
            state.predicted,
            state.interpolated,
            prespawned,
        );

        if controlled_by.is_some_and(|c| c.owner == sender_entity) {
            sender
                .prepare_typed_component_insert(entity, group_id, component_registry, &Controlled)
                .unwrap();
        }
    }
    spawn
}

/// Buffer entity despawn if an entity had [`Replicating`] and either:
/// - the [`Replicate`]/[`ReplicateState`] component is removed
/// - is despawned
/// - [`ReplicateLike`] is removed
///
/// We handle this in an observer because we need to access some information about the entity before it's despawned,
/// such as the [`ReplicationGroupId`].
/// TODO: we do not currently handle the case where an entity is [`ReplicateLike`] another entity
///   and that root entity is despawned? Maybe [`ReplicateLike`] should be a relationship?
///
/// Note that if the entity does not have [`Replicating`], we do not replicate the despawn
///
/// To despawn an entity without replicating it, you must first remove [`Replicating`] and then despawn the entity.
pub(crate) fn buffer_entity_despawn_replicate_remove(
    // this covers both cases
    trigger: On<Remove, (Replicate, ReplicationState, ReplicateLike)>,
    root_query: Query<&ReplicateLike>,
    // only replicate the despawn event if the entity still has Replicating at the time of despawn
    entity_query: Query<(&ReplicationGroup, &ReplicationState), With<Replicating>>,
    mut query: Query<(Entity, &mut ReplicationSender, &mut MessageManager)>,
    mut replicable_entities: ResMut<ReplicableRootEntities>,
) {
    let entity = trigger.entity;
    let root = root_query.get(entity).map_or(entity, |r| r.root);
    // TODO: use the child's ReplicationGroup if there is one that overrides the root's
    let Ok((group, replicate)) = entity_query.get(root) else {
        return;
    };
    replicable_entities.entities.swap_remove(&entity);
    debug!(?entity, ?replicate, "Buffering entity despawn");

    // TODO: if ReplicateLike is removed, we need to use the root entity's Replicate
    //  if Replicate is removed, we need to use the CachedReplicate (since Replicate is updated immediately via hook)
    //  for the root_entity and its ReplicateLike children

    // If the entity has NetworkVisibility, we only send the Despawn to the senders that have visibility
    // of this entity. Otherwise we send it to all senders that have the entity in their replicated_entities
    query
        .par_iter_many_unique_mut(replicate.per_sender_state.keys())
        .for_each(|(sender_entity, mut sender, manager)| {
            if replicate
                .per_sender_state
                .get(&sender_entity)
                .is_none_or(|v| !v.visibility.is_visible())
            {
                trace!(
                    ?entity,
                    ?sender_entity,
                    "Not sending despawn because the sender didn't have visibility of the entity"
                );
                return;
            }
            // convert the entity to a network entity (possibly mapped)
            let entity = manager.entity_mapper.to_remote(entity);
            // TODO: should we just buffer the despawn instead of sending it immediately, by adding the entity
            //  to sender.entities_to_despawn?
            sender.prepare_entity_despawn(entity, group.group_id(Some(entity)));
            trace!("preparing despawn to sender");
        });
}

/// This system sends updates for all components that were added or changed
/// Sends both ComponentInsert for newly added components and ComponentUpdates otherwise
///
/// Updates are sent only for any components that were changed since the most recent of:
/// - last time we sent an update for that group which got acked.
///
/// NOTE: cannot use ConnectEvents because they are reset every frame
#[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
fn replicate_component_update(
    current_tick: Tick,
    component_registry: &ComponentRegistry,
    unmapped_entity: Entity,
    // the mapped entity
    entity: Entity,
    component_kind: ComponentKind,
    component_data: Ptr,
    component_ticks: ComponentTicks,
    group_id: ReplicationGroupId,
    delta_compression: bool,
    replicate_once: bool,
    entity_map: &mut RemoteEntityMap,
    sender: &mut ReplicationSender,
    delta: Option<&DeltaManager>,
    spawn: bool,
) -> Result<(), ReplicationError> {
    let (mut insert, mut update) = (false, false);

    // send a component_insert for components that were newly added
    // or if the entity is newly replicated (for example for a new connection or if the entity
    // becomes visible)
    if spawn || sender.is_updated(component_ticks.added) {
        insert = true;
    } else {
        // do not send updates for these components, only inserts/removes
        if replicate_once {
            return Ok(());
        }
        // otherwise send an update for all components that changed since the
        // last update we have ack-ed
        update = true;
    }
    if insert || update {
        if insert {
            let writer = &mut sender.writer;
            trace!(?component_kind, ?entity, "Try to buffer component insert");
            if delta_compression {
                let delta = delta.expect("Delta compression on component {component_kind:?} is enabled, but no DeltaManager was provided");
                delta.store(
                    unmapped_entity,
                    current_tick,
                    component_kind,
                    component_data,
                    component_registry,
                );
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
            // trace!(?component_kind, ?entity, "Try to buffer component update");
            // check the send_tick, i.e. we will send all updates more recent than this tick
            let send_tick = sender.get_send_tick(group_id);

            // send the update for all changes newer than the last send bevy tick for the group
            if send_tick
                .is_none_or(|send_tick| component_ticks.is_changed(send_tick, sender.this_run))
            {
                trace!(
                    ?entity,
                    component = ?component_kind,
                    change_tick = ?component_ticks.changed,
                    ?send_tick,
                    current_tick = ?sender.this_run,
                    "prepare entity update changed"
                );
                if delta_compression {
                    let delta = delta.expect("Delta compression on component {component_kind:?} is enabled, but no DeltaManager was provided");
                    delta.store(
                        unmapped_entity,
                        current_tick,
                        component_kind,
                        component_data,
                        component_registry,
                    );
                    sender.prepare_delta_component_update(
                        unmapped_entity,
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
                    let writer = &mut sender.writer;
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
// TODO: you could have a case where you remove a component C, and then afterwards
//   modify the replication target, but we still send messages to the old components.
//   Maybe we should just add the components to a buffer?
pub(crate) fn buffer_component_removed(
    trigger: On<Remove>,
    // Query<&C, Or<With<ReplicateLike>, (With<Replicate>, With<ReplicationGroup>)>>
    query: Query<FilteredEntityRef>,
    registry: Res<ComponentRegistry>,
    root_query: Query<&ReplicateLike>,
    mut manager_query: Query<(Entity, &mut ReplicationSender, &mut MessageManager)>,
) {
    let entity = trigger.entity;
    let root = root_query.get(entity).map_or(entity, |r| r.root);
    let Ok(entity_ref) = query.get(root) else {
        return;
    };
    let Some(group) = entity_ref.get::<ReplicationGroup>() else {
        return;
    };
    let group_id = group.group_id(Some(root));
    let Some(replicate) = entity_ref.get::<ReplicationState>() else {
        return;
    };

    // Note: this is not needed because the SystemParamBuilder already makes sure that the entity
    //  must have both Replicate and Replicating
    // if !entity_ref.contains::<Replicating>() {
    //     return;
    // }

    manager_query
        .par_iter_many_unique_mut(replicate.per_sender_state.keys())
        .for_each(|(sender_entity, mut sender, manager)| {
            if replicate
                .per_sender_state
                .get(&sender_entity)
                .is_none_or(|v| !v.visibility.is_visible())
            {
                return;
            }
            // convert the entity to a network entity (possibly mapped)
            let entity = manager.entity_mapper.to_remote(entity);
            for component_id in trigger.trigger().components {
                // TODO: there is a bug in bevy where trigger.components() returns all the componnets that triggered
                //  Remove, not only the components that the observer is watching. This means that this could contain
                //  non replicated components, that we need to filter out
                // check if the component is disabled
                let Some(kind) = registry.component_id_to_kind.get(component_id) else {
                    continue;
                };
                let metadata = registry
                    .component_metadata_map
                    .get(kind)
                    .unwrap()
                    .replication
                    .as_ref()
                    .unwrap();
                let mut disable = metadata.config.disable;
                if let Some(overrides) = entity_ref
                    .get_by_id(metadata.overrides_component_id)
                    .and_then(|o| {
                        unsafe { o.deref::<ComponentReplicationOverrides<Replicate>>() }
                            .get_overrides(sender_entity)
                    })
                {
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
