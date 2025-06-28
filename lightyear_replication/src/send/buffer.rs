use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
use crate::registry::ComponentKind;
use crate::registry::registry::ComponentRegistry;
#[cfg(feature = "interpolation")]
use crate::send::components::{InterpolationTarget, ShouldBeInterpolated};
#[cfg(feature = "prediction")]
use crate::send::components::{PredictionTarget, ShouldBePredicted};
use crate::visibility::immediate::{NetworkVisibility, VisibilityState};
use bevy_ecs::change_detection::Mut;
use bevy_ecs::component::Components;
use bevy_ecs::prelude::*;
use bevy_ecs::{
    archetype::Archetypes,
    component::ComponentTicks,
    relationship::RelationshipTarget,
    system::SystemChangeTick,
    world::{FilteredEntityMut, FilteredEntityRef, OnRemove, Ref},
};
use bevy_ptr::Ptr;
use bytes::Bytes;
use tracing::{error, trace, trace_span};

use crate::components::ComponentReplicationOverrides;
use crate::control::{Controlled, ControlledBy};
use crate::send::archetypes::{ReplicatedArchetypes, ReplicatedComponent};
use crate::send::components::{
    CachedReplicate, Replicate, Replicating, ReplicationGroup, ReplicationGroupId,
};
use crate::send::sender::ReplicationSender;
use lightyear_connection::client::Connected;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{LocalTimeline, NetworkTimeline};
use lightyear_link::prelude::Server;
use lightyear_messages::MessageManager;
use lightyear_serde::entity_map::{RemoteEntityMap, SendEntityMap};
use lightyear_serde::writer::Writer;

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
    entity_query: Query<FilteredEntityMut>,
    mut manager_query: Query<
        (
            Entity,
            &mut ReplicationSender,
            &mut MessageManager,
            &LocalTimeline,
        ),
        With<Connected>,
    >,
    component_registry: Res<ComponentRegistry>,
    system_ticks: SystemChangeTick,
    archetypes: &Archetypes,
    components: &Components,
    mut replicated_archetypes: Local<ReplicatedArchetypes>,
) {
    replicated_archetypes.update(archetypes, components, component_registry.as_ref());

    // NOTE: this system doesn't handle delta compression, because we need to store a shared component history
    // for delta components, which is not possible when we start iterating through the senders first.
    // Maybe the easiest would be to simply store the component history for every tick where the sender is ready to send?
    // (this assumes that the senders are all sending at the same tick). Otherwise we store the component history for all
    // past ticks where the component changes.
    manager_query.par_iter_mut().for_each(
        |(sender_entity, mut sender, mut message_manager, timeline)| {
            let _span = trace_span!("replicate", sender = ?sender_entity).entered();
            let tick = timeline.tick();

            // enable split borrows
            let sender = &mut *sender;
            if !sender.send_timer.finished() {
                return;
            }
            // update the change ticks
            sender.last_run = sender.this_run;
            sender.this_run = system_ticks.this_run();

            // TODO: maybe this should be in a separate system in AfterBuffer?
            // run any possible tick cleanup
            sender.tick_cleanup(tick);

            trace!(
                this_run = ?sender.this_run,
                last_run = ?sender.last_run,
                "Starting buffer replication for sender {sender_entity:?}. Replicated entities: {:?}",
            sender.replicated_entities);
            // we iterate by index to avoid split borrow issues
            for i in 0..sender.replicated_entities.len() {
                let (&entity, &authority) = sender.replicated_entities.get_index(i).unwrap();
                if !authority {
                    trace!("Skipping entity {entity:?} because we don't have authority");
                    return;
                }
                let Ok(root_entity_ref) = entity_query.get(entity) else {
                    trace!("Replicated Entity {:?} not found in entity_query", entity);
                    return;
                };
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
                );
                if let Some(children) = root_entity_ref.get::<ReplicateLikeChildren>() {
                    for child in children.collection() {
                        let child_entity_ref = entity_query.get(*child).unwrap();
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
                        );
                    }
                }
            }

        },
    );
}

/// Alternative version of the `replicate` system that iterates through entities first instead of
/// senders. The benefit is that each component can be serialized only once per entity (and the bytes are shared per sender)
pub(crate) fn replicate_bis(
    // query &C + various replication components
    entity_query: Query<FilteredEntityMut>,
    mut sender_query: Query<
        (
            Entity,
            &mut ReplicationSender,
            &mut MessageManager,
            &LocalTimeline,
        ),
        With<Connected>,
    >,
    mut delta_query: Query<(&mut DeltaManager, &LocalTimeline), With<Server>>,
    component_registry: Res<ComponentRegistry>,
    system_ticks: SystemChangeTick,
    archetypes: &Archetypes,
    components: &Components,
    mut replicated_archetypes: Local<ReplicatedArchetypes>,
    // shared writer
    mut writer: Local<Writer>,
) {
    replicated_archetypes.update(archetypes, components, component_registry.as_ref());

    // TODO: if we use this design, it seems like we wouldn't need to store a list of replicated entities
    //  within each ReplicationSender

    sender_query.par_iter_mut().for_each(
        |(sender_entity, mut sender, message_manager, timeline)| {
            let tick = timeline.tick();

            // enable split borrows
            let sender = &mut *sender;
            if !sender.send_timer.finished() {
                return;
            }
            // update the change ticks
            sender.last_run = sender.this_run;
            sender.this_run = system_ticks.this_run();

            // TODO: maybe this should be in a separate system in AfterBuffer?
            // run any possible tick cleanup
            sender.tick_cleanup(tick);
        },
    );

    // TODO: in this design, we probably should find a way to not iterate through all entities if none of the senders are ready to send.
    //  Should we by default make all senders have the same send interval?

    // TODO: handle authority! the authority should be added on the replicate.senders EntityIndexMap

    // we can't iterate through entities in parallel because we need to mutate the senders
    let mut delta = delta_query
        .single_mut()
        .map(|(d, timeline)| (d, timeline.tick()))
        .ok();
    entity_query.iter().for_each(|entity_ref| {
        let entity = entity_ref.id();
        let _span = trace_span!("replicate", ?entity).entered();

        if entity_ref.contains::<Replicate>() {
            replicate_entity_bis(
                entity,
                &entity_ref,
                None,
                &mut sender_query,
                component_registry.as_ref(),
                &replicated_archetypes,
                &mut delta,
                &mut writer,
            );
        } else {
            let Some(replicate_like) = entity_ref.get::<ReplicateLike>() else {
                error!(
                    "Entity to replicate {:?} has no Replicate component and no ReplicateLike",
                    entity_ref.id()
                );
                return;
            };
            let Ok(root_entity_ref) = entity_query.get(replicate_like.root) else {
                error!(
                    "Root entity {:?} for ReplicateLike not found",
                    replicate_like.root
                );
                return;
            };
            replicate_entity_bis(
                entity,
                &root_entity_ref,
                Some(&(entity_ref, replicate_like.root)),
                &mut sender_query,
                component_registry.as_ref(),
                &replicated_archetypes,
                &mut delta,
                &mut writer,
            );
        }
    });
}

pub(crate) fn replicate_entity_bis(
    entity: Entity,
    root_entity_ref: &FilteredEntityRef,
    child_entity_ref: Option<&(FilteredEntityRef, Entity)>,
    sender_query: &mut Query<
        (
            Entity,
            &mut ReplicationSender,
            &mut MessageManager,
            &LocalTimeline,
        ),
        With<Connected>,
    >,
    component_registry: &ComponentRegistry,
    replicated_archetypes: &ReplicatedArchetypes,
    delta: &mut Option<(Mut<DeltaManager>, Tick)>,
    shared_writer: &mut Writer,
) {
    // get the value of the replication components
    let (
        group_id,
        priority,
        group_ready,
        replicate,
        cached_replicate,
        visibility,
        owned_by,
        entity_ref,
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
                // We use the root entity's Replicate/CachedReplicate component
                // SAFETY: we know that the root entity has the Replicate component
                root_entity_ref.get_ref::<Replicate>().unwrap(),
                root_entity_ref.get::<CachedReplicate>(),
                child_entity_ref
                    .get::<NetworkVisibility>()
                    .or_else(|| root_entity_ref.get::<NetworkVisibility>()),
                child_entity_ref
                    .get::<ControlledBy>()
                    .or_else(|| root_entity_ref.get::<ControlledBy>()),
                child_entity_ref,
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
                root_entity_ref.get_ref::<Replicate>().unwrap(),
                root_entity_ref.get::<CachedReplicate>(),
                root_entity_ref.get::<NetworkVisibility>(),
                root_entity_ref.get::<ControlledBy>(),
                root_entity_ref,
            )
        }
    };

    #[cfg(feature = "prediction")]
    let prediction_target = entity_ref
        .get::<PredictionTarget>()
        .or_else(|| root_entity_ref.get::<PredictionTarget>());
    #[cfg(feature = "interpolation")]
    let interpolation_target = entity_ref
        .get::<InterpolationTarget>()
        .or_else(|| root_entity_ref.get::<InterpolationTarget>());

    let replicated_components = replicated_archetypes
        .archetypes
        .get(&entity_ref.archetype().id())
        .unwrap();

    // the update will be 'insert' instead of update if the ReplicateOn component is new
    // or the HasAuthority component is new. That's because the remote cannot receive update
    // without receiving an action first (to populate the latest_tick on the replication-receiver)

    // TODO: do the entity mapping here!

    // send spawns/despawns immediately
    sender_query
        .par_iter_many_unique_mut(replicate.senders.as_slice())
        .for_each(
            |(sender_entity, mut sender, mut message_manager, timeline)| {
                let entity_mapper = &mut message_manager.entity_mapper;
                let sender = sender.as_mut();
                if !sender.send_timer.finished() {
                    return;
                }

                // b. add entity despawns from Visibility lost
                replicate_entity_despawn(
                    entity,
                    group_id,
                    entity_mapper,
                    visibility,
                    sender,
                    sender_entity,
                );

                // c. add entity spawns for Replicate changing
                let is_replicate_like_added =
                    child_entity_ref.is_some_and(|(child_entity_ref, _)| unsafe {
                        sender.is_updated(
                            child_entity_ref
                                .get_change_ticks::<ReplicateLike>()
                                .unwrap_unchecked()
                                .changed,
                        )
                    });
                replicate_entity_spawn(
                    entity,
                    group_id,
                    priority,
                    &replicate,
                    #[cfg(feature = "prediction")]
                    prediction_target,
                    #[cfg(feature = "interpolation")]
                    interpolation_target,
                    owned_by,
                    cached_replicate,
                    visibility,
                    entity_mapper,
                    component_registry,
                    sender,
                    sender_entity,
                    is_replicate_like_added,
                );
            },
        );

    // If the group is not set to send, skip this entity
    if !group_ready {
        return;
    }

    // d. all components that were added or changed and that are not disabled
    for ReplicatedComponent {
        id,
        kind,
        has_overrides,
    } in replicated_components
    {
        let is_map_entities = component_registry
            .serialize_fns_map
            .get(kind)
            .unwrap()
            .map_entities
            .is_some();
        let replication_metadata = component_registry.replication_map.get(kind).unwrap();
        let disable = replication_metadata.config.disable;
        let replicate_once = replication_metadata.config.replicate_once;
        let delta_compression = replication_metadata.config.delta_compression;
        // first check global overrides
        let overrides = (*has_overrides).then(|| {
            // TODO: the overrides should be merged from low importance to high importance (global -> root_entity -> child_entity)
            // SAFETY: we know that all overrides have the same shape
            unsafe {
                entity_ref
                    .get_by_id(replication_metadata.overrides_component_id)
                    .unwrap()
                    .deref::<ComponentReplicationOverrides<Replicate>>()
            }
        });
        if overrides.is_some_and(|o| o.is_disabled_for_all(disable)) {
            continue;
        }

        // if the global overrides don't disable the component, we will consider that it needs to be replicated!
        let Some(data) = entity_ref.get_by_id(*id) else {
            // component not present on entity, skip
            continue;
        };
        // we will consider that there probably is at least one sender that needs this component
        // so we will store it for delta-compression
        if delta_compression && let Some((delta_manager, shared_tick)) = delta {
            // NOTE: we are assuming that the tick of the entity having the DeltaManager is the same
            //  as the tick of the senders

            // store the component value in the delta manager
            delta_manager.data.store_component_value(
                entity,
                *shared_tick,
                *kind,
                data,
                component_registry,
            );
        }

        // we serialize it once for all senders if there is no `map_entities`.
        // if there is delta_compression, the serialization will depend on the last acked state, so we cannot
        // have a shared serialization
        let bytes = if !is_map_entities && !delta_compression {
            match component_registry.erased_serialize(
                data,
                shared_writer,
                *kind,
                &mut SendEntityMap::default(),
            ) {
                Err(e) => {
                    error!(
                        "Error serializing component {:?} for entity {:?}: {:?}",
                        kind, entity, e
                    );
                    continue;
                }
                _ => Some(shared_writer.split()),
            }
        } else {
            None
        };

        let component_ticks = entity_ref.get_change_ticks_by_id(*id).unwrap();
        let delta_manager = delta
            .as_ref()
            .map(|(delta_manager, _)| delta_manager.as_ref());
        sender_query
            .par_iter_many_unique_mut(replicate.senders.as_slice())
            .for_each(
                |(sender_entity, mut sender, mut message_manager, timeline)| {
                    if !sender.send_timer.finished() {
                        return;
                    }
                    // If we are using visibility and this sender is not visible, skip
                    if visibility.is_some_and(|vis| !vis.is_visible(sender_entity)) {
                        return;
                    }
                    let mut disable = disable;
                    let mut replicate_once = replicate_once;
                    if let Some(overrides) = overrides.and_then(|o| o.get_overrides(sender_entity))
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
                    };
                    if disable {
                        return;
                    }
                    let data = entity_ref.get_by_id(*id).unwrap();
                    let tick = timeline.tick();
                    let _ = replicate_component_update_shared(
                        tick,
                        component_registry,
                        entity,
                        *kind,
                        data,
                        bytes.clone(),
                        component_ticks,
                        &replicate,
                        group_id,
                        visibility.and_then(|v| v.clients.get(&sender_entity)),
                        delta_compression,
                        replicate_once,
                        &mut message_manager.entity_mapper,
                        sender.as_mut(),
                        delta_manager,
                    )
                    .inspect_err(|e| {
                        error!(
                            "Error replicating component {:?} update for entity {:?}: {:?}",
                            kind, entity, e
                        )
                    });
                },
            );
    }
}

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
) {
    // get the value of the replication components
    let (
        group_id,
        priority,
        group_ready,
        replicate,
        cached_replicate,
        visibility,
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
                // We use the root entity's Replicate/CachedReplicate component
                // SAFETY: we know that the root entity has the Replicate component
                root_entity_ref.get_ref::<Replicate>().unwrap(),
                root_entity_ref.get::<CachedReplicate>(),
                child_entity_ref
                    .get::<NetworkVisibility>()
                    .or_else(|| root_entity_ref.get::<NetworkVisibility>()),
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
                root_entity_ref.get_ref::<Replicate>().unwrap(),
                root_entity_ref.get::<CachedReplicate>(),
                root_entity_ref.get::<NetworkVisibility>(),
                root_entity_ref.get::<ControlledBy>(),
                root_entity_ref,
                false,
            )
        }
    };

    #[cfg(feature = "prediction")]
    let prediction_target = entity_ref
        .get::<PredictionTarget>()
        .or_else(|| root_entity_ref.get::<PredictionTarget>());
    #[cfg(feature = "interpolation")]
    let interpolation_target = entity_ref
        .get::<InterpolationTarget>()
        .or_else(|| root_entity_ref.get::<InterpolationTarget>());

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
        visibility,
        sender,
        sender_entity,
    );

    // c. add entity spawns for Replicate changing
    replicate_entity_spawn(
        entity,
        group_id,
        priority,
        &replicate,
        #[cfg(feature = "prediction")]
        prediction_target,
        #[cfg(feature = "interpolation")]
        interpolation_target,
        owned_by,
        cached_replicate,
        visibility,
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
    // If we are using visibility and this sender is not visible, skip
    if visibility.is_some_and(|vis| !vis.is_visible(sender_entity)) {
        return;
    }

    // d. all components that were added or changed and that are not disabled

    // NOTE: we pre-cache the list of components for each archetype to not iterate through
    //  all replicated components every time
    for ReplicatedComponent {
        id,
        kind,
        has_overrides,
    } in replicated_components
    {
        let replication_metadata = component_registry.replication_map.get(kind).unwrap();
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
            *kind,
            data,
            component_ticks,
            &replicate,
            group_id,
            visibility.and_then(|v| v.clients.get(&sender_entity)),
            delta_compression,
            replicate_once,
            entity_mapper,
            sender,
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
    mut senders: Query<&mut ReplicationSender>,
) {
    query
        .iter()
        .for_each(|(entity, group, replicate, cached_replicate)| {
            let group_id = group.group_id(Some(entity));
            cached_replicate
                .senders
                .difference(&replicate.senders)
                .for_each(|sender_entity| {
                    if let Ok(mut sender) = senders.get_mut(*sender_entity) {
                        trace!(
                            ?entity,
                            ?sender_entity,
                            ?replicate,
                            ?cached_replicate,
                            "Sending Despawn because replicate changed"
                        );
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
    if visibility
        .and_then(|v| v.clients.get(&sender_entity))
        .is_some_and(|s| s == &VisibilityState::Lost)
    {
        trace!(
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
pub(crate) fn replicate_entity_spawn(
    entity: Entity,
    group_id: ReplicationGroupId,
    priority: f32,
    replicate: &Ref<Replicate>,
    #[cfg(feature = "prediction")] prediction_target: Option<&PredictionTarget>,
    #[cfg(feature = "interpolation")] interpolation_target: Option<&InterpolationTarget>,
    controlled_by: Option<&ControlledBy>,
    cached_replicate: Option<&CachedReplicate>,
    network_visibility: Option<&NetworkVisibility>,
    entity_map: &mut RemoteEntityMap,
    component_registry: &ComponentRegistry,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
    is_replicate_like_added: bool,
) {
    // if the local entity is already mapped (for example because of authority transfer or PrePrediction), then
    // there is no need to send a Spawn
    if entity_map.get_remote(entity).is_some() {
        trace!(
            ?entity,
            ?group_id,
            "Not sending Spawn because entity is already mapped"
        );
        return;
    }

    // 1. replicate was added/updated and the sender was not in the previous Replicate's target
    let replicate_updated = sender.is_updated(replicate.last_changed())
        && cached_replicate.is_none_or(|cached| !cached.senders.contains(&sender_entity))
        && network_visibility.is_none_or(|vis| vis.is_visible(sender_entity));
    // 2. replicate was not updated but NetworkVisibility is gained for this sender
    let network_visibility_gained = network_visibility
        .and_then(|v| v.clients.get(&sender_entity))
        .is_some_and(|v| v == &VisibilityState::Gained);
    // 3. replicate-like was added and the the entity is visible for this sender
    let replicate_like_and_visible = is_replicate_like_added
        && network_visibility.is_none_or(|vis| vis.is_visible(sender_entity));
    if replicate_updated || network_visibility_gained || replicate_like_and_visible {
        trace!(
            ?entity,
            ?group_id,
            ?replicate,
            ?cached_replicate,
            ?network_visibility,
            ?replicate_updated,
            ?network_visibility_gained,
            ?replicate_like_and_visible,
            "Sending Spawn"
        );
        sender.prepare_entity_spawn(entity, group_id, priority);

        if controlled_by.is_some_and(|c| c.owner == sender_entity) {
            sender
                .prepare_typed_component_insert(entity, group_id, component_registry, &Controlled)
                .unwrap();
        }

        #[cfg(feature = "prediction")]
        if prediction_target.is_some_and(|p| p.senders.contains(&sender_entity)) {
            sender
                .prepare_typed_component_insert(
                    entity,
                    group_id,
                    component_registry,
                    &ShouldBePredicted,
                )
                .unwrap();
        }
        #[cfg(feature = "interpolation")]
        if interpolation_target.is_some_and(|p| p.senders.contains(&sender_entity)) {
            sender
                .prepare_typed_component_insert(
                    entity,
                    group_id,
                    component_registry,
                    &ShouldBeInterpolated,
                )
                .unwrap();
        }
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
    entity_query: Query<
        (
            &ReplicationGroup,
            &CachedReplicate,
            Option<&NetworkVisibility>,
        ),
        With<Replicating>,
    >,
    mut query: Query<(Entity, &mut ReplicationSender, &mut MessageManager)>,
) {
    let entity = trigger.target();
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
        .for_each(|(sender_entity, mut sender, manager)| {
            if network_visibility.is_some_and(|v| !v.is_visible(sender_entity)) {
                trace!(
                    ?entity,
                    ?sender_entity,
                    "Not sending despawn because the sender didn't have visibility of the entity"
                );
                return;
            }
            // convert the entity to a network entity (possibly mapped)
            let entity = manager.entity_mapper.to_remote(entity);
            sender.replicated_entities.swap_remove(&entity);
            sender.prepare_entity_despawn(entity, group.group_id(Some(entity)));
            trace!("preparing despawn to sender");
        });
}

/// This system sends updates for all components that were added or changed
/// Sends both ComponentInsert for newly added components and ComponentUpdates otherwise.
///
/// Updates are sent only for any components that were changed since the most recent of:
/// - last time we sent an update for that group which got acked.
///
/// NOTE: cannot use ConnectEvents because they are reset every frame
fn replicate_component_update_shared(
    current_tick: Tick,
    component_registry: &ComponentRegistry,
    mut entity: Entity,
    component_kind: ComponentKind,
    component_data: Ptr,
    component_bytes: Option<Bytes>,
    component_ticks: ComponentTicks,
    replicate: &Ref<Replicate>,
    group_id: ReplicationGroupId,
    visibility: Option<&VisibilityState>,
    delta_compression: bool,
    replicate_once: bool,
    entity_map: &mut RemoteEntityMap,
    sender: &mut ReplicationSender,
    delta: Option<&DeltaManager>,
) -> Result<(), ReplicationError> {
    let (mut insert, mut update) = (false, false);

    // send a component_insert for components that were newly added
    // or if we start replicating the entity
    // TODO: ideally we would use target.is_added(), but we do the trick of setting all the
    //  ReplicateToServer components to `changed` when the client first connects so that we replicate existing entities to the server
    //  That is why `force_insert = True` if ReplicateToServer is changed.
    if sender.is_updated(component_ticks.added)
        || sender.is_updated(replicate.last_changed())
        || visibility.is_some_and(|v| v == &VisibilityState::Gained)
    {
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
        // convert the entity to a network entity (possibly mapped)
        // NOTE: we have to apply the entity mapping here because we are sending the message directly to the Transport
        //  instead of relying on the MessageManagers' remote_entity_map. This is because using the MessageManager
        //  wouldn't give us back a MessageId.
        entity = entity_map.to_remote(entity);

        if insert {
            let writer = &mut sender.writer;
            trace!(?component_kind, ?entity, "Try to buffer component insert");
            let raw_data = if delta_compression {
                // TODO: would there be a way to serialize this only once as well?
                // SAFETY: the component_data corresponds to the kind
                unsafe {
                    component_registry.serialize_diff_from_base_value(
                        component_data,
                        writer,
                        component_kind,
                        &mut entity_map.local_to_remote,
                    )?
                };
                writer.split()
            } else if let Some(component_bytes) = component_bytes {
                component_bytes
            } else {
                component_registry.erased_serialize(
                    component_data,
                    writer,
                    component_kind,
                    &mut entity_map.local_to_remote,
                )?;
                writer.split()
            };
            sender.prepare_component_insert(entity, group_id, raw_data);
        } else {
            trace!(?component_kind, ?entity, "Try to buffer component update");
            // check the send_tick, i.e. we will send all updates more recent than this tick
            let send_tick = sender.get_send_tick(group_id);

            // send the update for all changes newer than the last send bevy tick for the group
            if send_tick
                .is_none_or(|send_tick| component_ticks.is_changed(send_tick, sender.this_run))
            {
                trace!(
                    ?entity,
                    ?component_kind,
                    change_tick = ?component_ticks.changed,
                    ?send_tick,
                    ?current_tick,
                    current_bevy_tick = ?sender.this_run,
                    "Prepare component update"
                );
                if delta_compression && let Some(delta) = delta {
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
                    let raw_data = if let Some(component_bytes) = component_bytes {
                        component_bytes
                    } else {
                        component_registry.erased_serialize(
                            component_data,
                            &mut sender.writer,
                            component_kind,
                            &mut entity_map.local_to_remote,
                        )?;
                        sender.writer.split()
                    };
                    sender.prepare_component_update(entity, group_id, raw_data);
                }
            }
        }
    }
    Ok(())
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
    visibility: Option<&VisibilityState>,
    delta_compression: bool,
    replicate_once: bool,
    entity_map: &mut RemoteEntityMap,
    sender: &mut ReplicationSender,
) -> Result<(), ReplicationError> {
    let (mut insert, mut update) = (false, false);

    // send a component_insert for components that were newly added
    // or if we start replicating the entity
    // TODO: ideally we would use target.is_added(), but we do the trick of setting all the
    //  ReplicateToServer components to `changed` when the client first connects so that we replicate existing entities to the server
    //  That is why `force_insert = True` if ReplicateToServer is changed.
    if sender.is_updated(component_ticks.added)
        || sender.is_updated(replicate.last_changed())
        || visibility.is_some_and(|v| v == &VisibilityState::Gained)
    {
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
        // convert the entity to a network entity (possibly mapped)
        // NOTE: we have to apply the entity mapping here because we are sending the message directly to the Transport
        //  instead of relying on the MessageManagers' remote_entity_map. This is because using the MessageManager
        //  wouldn't give us back a MessageId.
        entity = entity_map.to_remote(entity);

        if insert {
            let writer = &mut sender.writer;
            trace!(?component_kind, ?entity, "Try to buffer component insert");
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
            trace!(?component_kind, ?entity, "Try to buffer component update");
            // check the send_tick, i.e. we will send all updates more recent than this tick
            let send_tick = sender.get_send_tick(group_id);

            // send the update for all changes newer than the last send bevy tick for the group
            if send_tick
                .is_none_or(|send_tick| component_ticks.is_changed(send_tick, sender.this_run))
            {
                trace!(
                    change_tick = ?component_ticks.changed,
                    ?send_tick,
                    current_tick = ?sender.this_run,
                    "prepare entity update changed check"
                );
                // trace!(
                //     ?entity,
                //     component = ?kind,
                //     tick = ?self.tick_manager.tick(),
                //     "Updating single component"
                // );
                if delta_compression {
                    // TODO: handle delta compression!
                    let delta = DeltaManager::default();
                    sender.prepare_delta_component_update(
                        entity,
                        group_id,
                        component_kind,
                        component_data,
                        component_registry,
                        &delta,
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
        .for_each(|(sender_entity, mut sender, manager)| {
            // convert the entity to a network entity (possibly mapped)
            let entity = manager.entity_mapper.to_remote(entity);
            for component_id in trigger.components() {
                // TODO: there is a bug in bevy where trigger.components() returns all the componnets that triggered
                //  OnRemove, not only the components that the observer is watching. This means that this could contain
                //  non replicated components, that we need to filter out
                // check if the component is disabled
                let Some(kind) = registry.component_id_to_kind.get(component_id) else {
                    continue;
                };
                let metadata = registry.replication_map.get(kind).unwrap();
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
