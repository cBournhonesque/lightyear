//! Alternative version of the `replicate` system where we replicate a component only
//! once. The serialized bytes are shared between all senders.
use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::hierarchy::ReplicateLike;
use crate::registry::ComponentKind;
use crate::registry::registry::ComponentRegistry;
#[cfg(feature = "interpolation")]
use crate::send::components::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::send::components::PredictionTarget;
use crate::visibility::immediate::{NetworkVisibility, VisibilityState};
use bevy_ecs::change_detection::Mut;
use bevy_ecs::component::Components;
use bevy_ecs::prelude::*;
use bevy_ecs::{
    archetype::Archetypes,
    component::ComponentTicks,
    system::SystemChangeTick,
    world::{FilteredEntityMut, FilteredEntityRef, Ref},
};
use bevy_ptr::Ptr;
use bytes::Bytes;
use tracing::{error, trace, trace_span};

use crate::components::ComponentReplicationOverrides;
use crate::control::ControlledBy;
use crate::send::archetypes::{ReplicatedArchetypes, ReplicatedComponent};
use crate::send::components::{CachedReplicate, Replicate, ReplicationGroup, ReplicationGroupId};
use crate::send::sender::ReplicationSender;
use lightyear_connection::client::Connected;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{LocalTimeline, NetworkTimeline};
use lightyear_link::prelude::Server;
use lightyear_messages::MessageManager;
use lightyear_serde::entity_map::{RemoteEntityMap, SendEntityMap};
use lightyear_serde::writer::Writer;

/// Alternative version of the `replicate` system that iterates through entities first instead of
/// senders. The benefit is that each component can be serialized only once per entity (and the bytes are shared per sender)
pub fn replicate_bis(
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

pub fn replicate_entity_bis(
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
                super::buffer::replicate_entity_despawn(
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
                super::buffer::replicate_entity_spawn(
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
            .component_metadata_map
            .get(kind)
            .unwrap()
            .serialization
            .as_ref()
            .is_some_and(|s| s.map_entities.is_some());
        let replication_metadata = component_registry
            .component_metadata_map
            .get(kind)
            .unwrap()
            .replication
            .as_ref()
            .unwrap();
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
            delta_manager.store(entity, *shared_tick, *kind, data, component_registry);
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
    unmapped_entity: Entity,
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
        let entity = entity_map.to_remote(unmapped_entity);

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
