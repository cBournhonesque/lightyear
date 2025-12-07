use crate::components::ComponentReplicationOverrides;
use crate::control::{Controlled, ControlledBy};
use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
use crate::messages::actions::SpawnAction;
use crate::messages::serialized_data::{
    ByteRange, MessageWrite, SerializedData, WritableComponent,
};
use crate::prelude::{NetworkVisibility, ReplicationFrequency, ReplicationState};
use crate::prespawn::PreSpawned;
use crate::registry::ComponentKind;
use crate::registry::component_mask::ComponentMask;
use crate::registry::registry::ComponentRegistry;
use crate::send::archetypes::{ReplicatedArchetype, ReplicatedArchetypes, ReplicatedComponent};
use crate::send::client_pools::ClientPools;
#[cfg(feature = "interpolation")]
use crate::send::components::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::send::components::PredictionTarget;
use crate::send::components::{Replicate, Replicating, ReplicationGroup};
use crate::send::plugin::ReplicableRootEntities;
use crate::send::query::ReplicationQuery;
use crate::send::sender::ReplicationSender;
use crate::send::sender_ticks::EntityTicks;
use crate::visibility::immediate::VisibilityState;
use alloc::vec::Vec;
use bevy_ecs::archetype::{Archetype, ArchetypeEntity};
use bevy_ecs::component::Components;
use bevy_ecs::component::Tick as BevyTick;
use bevy_ecs::entity::{EntityHash, UniqueEntitySlice};
use bevy_ecs::prelude::*;
use bevy_ecs::storage::TableRow;
use bevy_ecs::world::FilteredEntityMut;
use bevy_ecs::world::unsafe_world_cell::UnsafeEntityCell;
use bevy_ecs::{
    archetype::Archetypes, component::ComponentTicks, relationship::RelationshipTarget,
    system::SystemChangeTick, world::FilteredEntityRef,
};
use bevy_platform::collections::hash_map::Entry;
use bevy_ptr::Ptr;
use bevy_time::{Time, Timer, TimerMode};
use core::ops::Range;
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::host::HostClient;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::LocalTimeline;
use lightyear_link::prelude::Server;
use lightyear_link::server::LinkOf;
use lightyear_messages::MessageManager;
use lightyear_serde::SerializationError;
use lightyear_serde::entity_map::RemoteEntityMap;
#[cfg(feature = "metrics")]
use lightyear_utils::metrics::DormantTimerGauge;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, error, info, info_span, trace, trace_span, warn};

pub const REPLICATION_PARALLELISM: usize = 10;
// - we need the same frequency for all! Have the timer in a global resource?

// for each entity chunk (10 groups)
//    - write raw bytes in a serialize data, + information about the ranges for each client
// then for each client in parallel:
//    -

pub(crate) fn should_run_replication(
    time: Res<Time>,
    mut metadata: ResMut<ReplicationMetadata>,
    system_ticks: SystemChangeTick,
) -> bool {
    metadata.timer.tick(time.delta());
    if metadata.timer.just_finished() {
        metadata.change_tick = system_ticks.this_run();
        true
    } else {
        false
    }
}

#[derive(Resource)]
pub struct ReplicationMetadata {
    // change tick of the replication system (stored here so that multiple replication systems
    // can use the exact same change_tick)
    pub(crate) change_tick: BevyTick,
    // timer that tracks how often we should run the replication systems
    pub(crate) timer: Timer,
}

impl ReplicationMetadata {
    fn new(replication_frequency: Duration) -> Self {
        Self {
            change_tick: BevyTick::default(),
            timer: Timer::new(replication_frequency, TimerMode::Repeating),
        }
    }
}

// TODO: instead of the ranges, we could:
// - still serialize in a single buffer
// - then split the bytes and store them in HashMap<entity> ?

/// System that buffers any kind of replication messages
/// into each [`ReplicationSender`]'s `pending_actions` and `pending_updates`.
pub(crate) fn replicate(
    // query &C + various replication components
    // we know that we always query Replicate from the parent
    mut query: ReplicationQuery,
    mut sender_query: Query<
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
    component_registry: Res<ComponentRegistry>,
    metadata: Res<ReplicationMetadata>,
    archetypes: &Archetypes,
    components: &Components,
    mut serialized: ResMut<SerializedData>,
    mut pools: ResMut<ClientPools>,
    mut replicated_archetypes: Local<ReplicatedArchetypes>,
) {
    let tick = Tick(0);
    #[cfg(feature = "metrics")]
    let _timer = DormantTimerGauge::new("replication/buffer");

    replicated_archetypes.update(archetypes, components, component_registry.as_ref());

    // we iterate through entities first to query for components only once, and

    // TODO: first pass in parallel inside ReplicationGroups, iterate through ReplicationGroupChildren, and then ReplicateLikeChildren
    // components with InReplicationGroup.

    // TODO: do it in parallel.
    //  maybe get the writer for the chunk via thread_local?
    // now replicate via ReplicateLike roots:
    // - components with Replicate and no ReplicateLike
    // replicable_entities.entities.iter().chunk()
    replicated_archetypes.root_archetypes.iter().for_each(
        |(archetype_id, replicated_archetype)| {
            // SAFETY: we know that the archetype_id is valid
            let archetype = unsafe { archetypes.get(*archetype_id).unwrap_unchecked() };

            for archetype_entity in archetype.entities() {
                let entity = archetype_entity.id();
                let _root_span = trace_span!("entity", ?entity).entered();
                let entity_cell = query.cell(entity);
                replicate_entity(
                    archetype,
                    tick,
                    entity_cell,
                    None,
                    &query,
                    &mut sender_query,
                    &component_registry,
                    &replicated_archetype,
                    &metadata,
                    serialized.as_mut(),
                    pools.as_mut(),
                );
                if replicated_archetype.has_replicate_like_children {
                    // SAFETY: we checked above that the entity has the component
                    for child_entity in unsafe {
                        entity_cell
                            .get::<ReplicateLikeChildren>()
                            .unwrap_unchecked()
                    }
                    .collection()
                    {
                        let _child_span = trace_span!("child", ?child_entity).entered();
                        let child_entity_cell = query.cell(*child_entity);
                        let child_archetype = child_entity_cell.archetype();
                        // SAFETY: we know the archetype exists since we include all archetypes with ReplicateLike
                        let replicated_archetype = unsafe {
                            replicated_archetypes
                                .child_archetypes
                                .get(&child_archetype.id())
                                .unwrap_unchecked()
                        };
                        replicate_entity(
                            child_archetype,
                            tick,
                            child_entity_cell,
                            Some(entity_cell),
                            &query,
                            &mut sender_query,
                            component_registry.as_ref(),
                            &replicated_archetype,
                            &metadata,
                            serialized.as_mut(),
                            pools.as_mut(),
                        );
                    }
                }
            }
        },
    );

    // TODO:
    // // Drain all entities that should be despawned because Replicate changed and they are not in the new
    // // Replicate's senders list anymore
    // sender.prepare_entity_despawns();
}

#[derive(Debug)]
pub(crate) struct StateMetadata {
    // child: will check if ReplicateLike was added since the sender's last
    should_spawn: bool,
    // use child's if NetworkVisibility is present
    visible: bool,
    // this is for despawns that are NetworkVisibility-related
    lost_visibility: bool,
    // use child's if PredictionTarget is present
    #[cfg(feature = "prediction")]
    predicted: bool,
    // use child's if InterpolationTarget is present
    #[cfg(feature = "interpolation")]
    interpolated: bool,
}

#[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
pub(crate) fn replicate_entity(
    archetype: &Archetype,
    tick: Tick,
    entity_cell: UnsafeEntityCell,
    root_entity_cell: Option<UnsafeEntityCell>,
    // used to fetch replication component values from the entity
    query: &ReplicationQuery,
    mut sender_query: &mut Query<
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
    component_registry: &ComponentRegistry,
    replicated_archetype: &ReplicatedArchetype,
    replication_metadata: &ReplicationMetadata,
    mut serialized: &mut SerializedData,
    mut pools: &mut ClientPools,
) -> Result<(), ReplicationError> {
    let entity = entity_cell.id();

    let is_child = root_entity_cell.is_some();
    let has_child = replicated_archetype.has_replicate_like_children;
    let root_entity_cell = root_entity_cell.unwrap_or(entity_cell);

    // check if the entity needs to be replicated to any of the senders
    // TODO: provide a re-usable vec to avoid allocation
    let mut senders_needed = Vec::default();

    // cache to make sure that we only serialize the entity once
    let mut entity_cache: Option<ByteRange> = None;

    // check if the entity should be replicated.
    for (sender_entity, mut sender, _, _, _, _) in sender_query.iter_mut() {
        // SAFETY: the entity is guaranteed to have this component
        let Some(state) = unsafe {
            root_entity_cell
                .get::<ReplicationState>()
                .unwrap_unchecked()
        }
        .per_sender_state
        .get(&sender_entity) else {
            return Ok(());
        };
        // TODO: fill this
        let is_replicate_like_added = False;
        let has_network_visibility = replicated_archetype.has_network_visibility;
        let state_metadata = if let Some(child_state) =
            // SAFETY: we have added read access to this component
            is_child
                .then(|| unsafe { entity_cell.get::<ReplicationState>() })
                .flatten()
        {
            // this could be done (for example if parent has [1, 2] but child added PredictionTarget for [1],
            // there is no state for 2 on the child)
            let child_state = child_state.per_sender_state.get(&sender_entity);

            // if replicate is set, then we need explicit authority on the child. (we don't delegate to the parent)
            let authority = if entity_cell.contains::<Replicate>() {
                child_state.is_some_and(|a| a.authority == Some(true))
            } else {
                // use child's if authority is not None (so it has been explicitly set either because Replicate was added
                // explicitly on the child, or because of an authority transfer)
                if let Some(a) = child_state.and_then(|s| s.authority) {
                    a
                } else {
                    state.authority == Some(true)
                }
            };
            if !authority {
                return Ok(());
            }

            // if NetworkVisibility was added directly on the child, we use the child's values
            let (should_spawn, visible, lost_visibility) =
                if entity_cell.contains::<NetworkVisibility>() {
                    // replicate-like was added and the entity is visible for this sender
                    // (this is important in cases where the parent was already spawned so should_spawn is false, but we just
                    //  added a child entity)
                    let raw = child_state.map_or(VisibilityState::Default, |s| s.visibility);
                    let visible = raw.is_visible(true);
                    (
                        (visible && is_replicate_like_added) || (raw == VisibilityState::Gained),
                        visible,
                        raw == VisibilityState::Lost,
                    )
                } else {
                    let visible = state.visibility.is_visible(has_network_visibility);
                    (
                        (visible && is_replicate_like_added)
                            || state.visibility == VisibilityState::Gained,
                        visible,
                        state.visibility == VisibilityState::Lost,
                    )
                };
            StateMetadata {
                should_spawn,
                visible,
                lost_visibility,
                // TODO: provide the component_id to avoid an extra check?
                #[cfg(feature = "prediction")]
                predicted: if entity_cell.contains::<PredictionTarget>() {
                    child_state.is_some_and(|c| c.predicted)
                } else {
                    state.predicted
                },
                #[cfg(feature = "interpolation")]
                interpolated: if entity_cell.contains::<InterpolationTarget>() {
                    child_state.is_some_and(|c| c.interpolated)
                } else {
                    state.interpolated
                },
            }
        } else {
            if state.authority.is_none_or(|a| !a) {
                return Ok(());
            }
            let visible = state.visibility.is_visible(has_network_visibility);
            // 1. the sender got added to the list of senders for this entity's Replicate but we haven't spawned the entity
            //    yet for this sender
            //    Checking if `Replicate` is changed is not enough, because we don't to re-send Spawn for entities we have
            //    already replicated.
            // 2. If VisibilityState::Gained, we ignore state.spawned (since we need to re-send a spawn)
            // 3. replicate-like was added and the entity is visible for this sender
            //    (this is important in cases where the parent was already spawned so should_spawn is false, but we just
            //    added a child entity)
            let should_spawn = (visible && (!state.spawned || is_replicate_like_added))
                || state.visibility == VisibilityState::Gained;
            StateMetadata {
                should_spawn,
                visible,
                // only send a despawn if we previously sent a spawn
                lost_visibility: (state.visibility == VisibilityState::Lost) && state.spawned,
                #[cfg(feature = "prediction")]
                predicted: state.predicted,
                #[cfg(feature = "interpolation")]
                interpolated: state.interpolated,
            }
        };

        // add entity despawns from Visibility lost
        if state_metadata.lost_visibility
            && let Some(entity_ticks) = sender.sender_ticks.entities.remove(&entity)
        {
            // Write despawn only if the entity was previously sent because
            // spawn and despawn could happen during the same tick.
            trace!("writing despawn for `{entity}` for client `{sender_entity}`");
            let entity_range = entity.write(&mut serialized)?;
            sender.pending_actions.add_despawn(entity_range.clone());
            pools.recycle_components(entity_ticks.components);
            return Ok(());
        }

        if !state_metadata.visible {
            return Ok(());
        }

        // add entity spawns (Replicate changed, NetworkVisibility::Gained, ReplicateLike added)
        if state_metadata.should_spawn {
            replicate_entity_spawn(
                entity,
                &state_metadata,
                prespawned,
                owned_by,
                sender.as_mut(),
                sender_entity,
                serialized,
                &mut entity_cache,
            )?;
        }

        senders_needed.push(sender_entity);
        // TODO: here all actions are sent in the same message. do we want that?
        sender.pending_actions.start_entity_changes();
        sender.pending_updates.start_entity();
    }

    // stop now if the entity isn't relevant to any sender
    if senders_needed.is_empty() {
        // TODO: or do we need to finalize the write in any way??
        return Ok(());
    }

    // TODO: re-use a vec for this! also maybe use a vec with all senders, not just the needed ones!
    // cache the query results to avoid re-running it multiple times
    // SAFETY: the senders are guaranteed to be unique
    let senders_unique = unsafe { UniqueEntitySlice::from_slice_unchecked(&senders_needed) };
    let mut senders_data: Vec<_> = sender_query.iter_many_unique_mut(senders_unique).collect();

    for component in replicated_archetype.components {
        replicate_component(
            component,
            entity,
            entity_cell,
            is_child,
            has_child,
            archetype,
            query,
            component_registry,
            replication_metadata,
            serialized,
            pools,
            &mut senders_data,
            &mut entity_cache,
        )?;
    }

    for (sender_entity, sender, _, _, _, _) in senders_data.iter_mut() {
        let entity_ticks = sender.sender_ticks.entities.entry(entity);
        let new_for_client = matches!(entity_ticks, Entry::Vacant(_));
        if new_for_client || sender.pending_actions.changed_entity_added()
        // TODO: how do we check that there is a removal for this entity?
        // || removal_buffer.contains_key(&entity.id())
        {
            // If there is any insertion, removal, or it's a new entity for a client, include all mutations
            // into update message and bump the last acknowledged tick to keep entity updates atomic.
            if sender.pending_updates.entity_added() {
                trace!(
                    "merging updates for `{}` with actions for client `{sender_entity}`",
                    entity
                );
                sender
                    .pending_actions
                    .take_added_entity(&mut pools, &mut sender.pending_updates);
            }

            update_ticks(
                entity_ticks,
                &mut pools,
                replication_metadata.change_tick,
                tick,
                sender.pending_actions.take_changed_components(),
            );
        }

        if new_for_client && !sender.pending_actions.changed_entity_added() {
            trace!("writing empty `{}` for client `{sender_entity}`", entity);

            // Force-write new entity even if it doesn't have any components.
            let entity_range = entity.write_cached(&mut serialized, &mut entity_cache)?;
            sender
                .pending_actions
                .add_changed_entity(&mut pools, entity_range);
        }
    }

    Ok(())
}

struct ComponentOverrides {
    // index of the component in the ComponentRegistry
    component_index: usize,
    disable: bool,
    replicate_once: bool,
}

pub(crate) fn get_component_overrides(
    component_registry: &ComponentRegistry,
    component: &ReplicatedComponent,
    entity_cell: UnsafeEntityCell,
    sender_entity: Entity,
) -> ComponentOverrides {
    let component_metadata = component_registry
        .component_metadata_map
        .get(&component.kind)
        .unwrap();
    let replication_metadata = component_metadata.replication.as_ref().unwrap();
    let mut disable = replication_metadata.config.disable;
    let mut replicate_once = replication_metadata.config.replicate_once;
    if component.has_overrides {
        // TODO: get ComponentReplicationOverrides using root entity
        // SAFETY: see below
        if let Some(overrides) = unsafe {
            // SAFETY: we granted read access to the overrides for this entity
            entity_cell
                .get_by_id(replication_metadata.overrides_component_id)
                .unwrap()
                // SAFETY: we know that all overrides have the same shape
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
    let net_id = *unsafe {
        component_registry
            .kind_map
            .net_id(&component.kind)
            .unwrap_unchecked()
    };
    ComponentOverrides {
        component_index: net_id as usize,
        disable,
        replicate_once,
    }
}

pub(crate) fn replicate_component(
    component: ReplicatedComponent,
    entity: Entity,
    entity_cell: UnsafeEntityCell,
    is_child: bool,
    has_child: bool,
    archetype: &Archetype,
    query: &ReplicationQuery,
    component_registry: &ComponentRegistry,
    replication_metadata: &ReplicationMetadata,
    serialized: &mut SerializedData,
    pools: &mut ClientPools,
    mut senders: &mut Vec<(
        Entity,
        Mut<ReplicationSender>,
        Mut<MessageManager>,
        &LocalTimeline,
        Option<&DeltaManager>,
        Option<&LinkOf>,
    )>,
    entity_cache: &mut Option<ByteRange>,
) -> Result<(), ReplicationError> {
    let system_tick = replication_metadata.change_tick;
    let table_row = entity_cell.location().table_row;
    // SAFETY: component and storage were obtained from this archetype.
    let (ptr, component_ticks) = unsafe {
        query.get_component_unchecked(
            entity,
            table_row,
            archetype.table_id(),
            component.storage_type,
            component.id,
        )
    };

    // SAFETY: `fns` and `ptr` were created for the same component type.
    let writable_component = WritableComponent {
        ptr,
        kind: &component.kind,
        registry: component_registry,
    };

    let mut component_cache = None;
    senders.iter_mut().try_for_each(
        |(sender_entity, sender, message_manager, local_timeline, delta_manager, link_of)| {
            let ComponentOverrides {
                component_index,
                disable,
                replicate_once,
            } = get_component_overrides(
                component_registry,
                &component,
                entity_cell,
                *sender_entity,
            );
            if disable {
                return Ok(());
            }

            if let Some(entity_ticks) = sender.sender_ticks.entities.get(&entity)
                && entity_ticks.components.contains(component_index)
            {
                // update
                if !replicate_once
                // TODO: need to change this to either ack_tick or send_tick!
                && component_ticks.is_changed(entity_ticks.system_tick, system_tick)
                {
                    trace!(
                        "writing `{:?}` update for {:?} for sender `{sender_entity}`",
                        entity, component.kind,
                    );

                    if !sender.pending_updates.entity_added() {
                        let entity_range = entity.write_cached(serialized, entity_cache)?;
                        sender.pending_updates.add_entity(
                            pools,
                            entity,
                            is_child,
                            has_child,
                            entity_range,
                        );
                    }
                    let component_range =
                        writable_component.write_cached(serialized, &mut component_cache)?;
                    sender.pending_updates.add_component(component_range);
                }
            } else {
                trace!(
                    "writing `{:?}` insert for `{:?}` for sender `{sender_entity}`",
                    entity, component.kind
                );

                if !sender.pending_actions.changed_entity_added() {
                    let entity_range = entity.write_cached(serialized, entity_cache)?;
                    sender
                        .pending_actions
                        .add_changed_entity(pools, entity_range);
                }
                let component_range =
                    writable_component.write_cached(serialized, &mut component_cache)?;
                sender
                    .pending_actions
                    .add_inserted_component(component_range, component_index);
            }
            Ok::<(), ReplicationError>(())
        },
    )
}

fn update_ticks(
    entity_ticks: Entry<Entity, EntityTicks, EntityHash>,
    pools: &mut ClientPools,
    system_tick: BevyTick,
    server_tick: Tick,
    components: ComponentMask,
) {
    match entity_ticks {
        Entry::Occupied(entry) => {
            let entity_ticks = entry.into_mut();
            entity_ticks.system_tick = system_tick;
            entity_ticks.server_tick = server_tick;
            entity_ticks.components |= &components;
            pools.recycle_components(components);
        }
        Entry::Vacant(entry) => {
            entry.insert(EntityTicks {
                server_tick,
                system_tick,
                components,
            });
        }
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
    state: &StateMetadata,
    #[allow(unused_mut)] mut prespawned: Option<&PreSpawned>,
    controlled_by: Option<&ControlledBy>,
    sender: &mut ReplicationSender,
    sender_entity: Entity,
    mut serialized: &mut SerializedData,
    mut entity_cache: &mut Option<ByteRange>,
) -> Result<(), ReplicationError> {
    // normal-case: server sends to client. just include server entity (client will
    //   spawn and map on receiver side)
    // pre-spawned: server sends with signature. client will find corresponding entity
    //   and map on receiver side)
    // authority-change: client spawned and sent to server. Server spawned and has the
    //   mapping. Authority transferred to server. Server will send all other components with non-mapped entities (so we can
    //   serialize only once), so server needs to only include its local entity. On the receiver side the client will find that it already has the entity in its entity map, and should apply the
    // mapping there.

    // make sure to not send again if we already did
    if sender.sender_ticks.entities.contains_key(&entity) {
        return Ok(());
    }
    #[cfg(feature = "interpolation")]
    if state.interpolated {
        // if the entity is interpolated, we don't want to Prespawn it
        prespawned = None;
    }
    let action = SpawnAction {
        #[cfg(feature = "prediction")]
        predicted,
        #[cfg(feature = "interpolation")]
        interpolated,
        // TODO: handle controlled on receiver side
        controlled: controlled_by.is_some_and(|c| c.owner == sender_entity),
        prespawn: prespawned.and_then(|p| p.hash),
    };
    // TODO: maybe only send PreSpawned to some clients.
    // TODO: can we cache the entity at least?
    let entity_range = action.write_cached(&mut serialized, entity_cache)?;
    let spawn_range = action.write(&mut serialized)?;

    // TODO: add entity in pending_actions!
    sender.pending_actions.add_mapping(spawn_range);

    // TODO: mark that this entity has been spawned to this sender!
    debug!(?entity, ?state, "Sending Spawn");
    Ok(())
}

/// Buffer entity despawn if an entity had [`Replicating`] and either:
/// - the [`Replicate`]/[`ReplicateState`] component is removed
/// - is despawned
/// - [`ReplicateLike`] is removed
///
/// TODO: we do not currently handle the case where an entity is [`ReplicateLike`] another entity
///   and that root entity is despawned?
///
/// Note that if the entity does not have [`Replicating`], we do not replicate the despawn
///
/// To despawn an entity without replicating it, you must first remove [`Replicating`] and then despawn the entity.
pub(crate) fn buffer_entity_despawn_replicate_remove(
    // this covers both cases
    trigger: On<Remove, (Replicate, ReplicationState, ReplicateLike)>,
    root_query: Query<&ReplicateLike>,
    // only replicate the despawn event if the entity still has Replicating at the time of despawn
    entity_query: Query<(&ReplicationState, Has<NetworkVisibility>), With<Replicating>>,
    mut query: Query<(Entity, &mut ReplicationSender, &mut MessageManager)>,
    mut replicable_entities: ResMut<ReplicableRootEntities>,
    mut serialized: ResMut<SerializedData>,
    mut pools: ResMut<ClientPools>,
) -> Result<(), BevyError> {
    let entity = trigger.entity;
    let root = root_query.get(entity).map_or(entity, |r| r.root);
    let Ok((replicate, has_network_visibility)) = entity_query.get(root) else {
        return Ok(());
    };
    replicable_entities.entities.swap_remove(&entity);
    debug!(?entity, ?replicate, "Buffering entity despawn");

    // TODO: if ReplicateLike is removed, we need to use the root entity's Replicate
    //  for the root_entity and its ReplicateLike children

    // only send the despawn to senders that have visibility over the entity.
    let senders = replicate.per_sender_state.keys().filter(|sender_entity| {
        replicate
            .per_sender_state
            .get(*sender_entity)
            .is_some_and(|s| s.visibility.is_visible(has_network_visibility))
    });
    let entity_range = entity.write(&mut serialized)?;
    query
        .iter_many_unique_mut(senders)
        .for_each(|(sender_entity, mut sender, manager)| {
            if let Some(entity_ticks) = sender.sender_ticks.entities.remove(&entity) {
                // Write despawn only if the entity was previously sent because
                // spawn and despawn could happen during the same tick.
                trace!("writing despawn for `{entity}` for sender `{sender_entity}`");
                sender.pending_actions.add_despawn(entity_range.clone());
                pools.recycle_components(entity_ticks.components);
            }
        });
    Ok(())
}

// TODO: does this also trigger when the entity gets despawned???

/// Send component remove message when a replicated component gets removed
// TODO: you could have a case where you remove a component C, and then afterwards
//   modify the replication target, but we still send messages to the old components.
//   Maybe we should just add the components to a buffer?
pub(crate) fn buffer_component_removed(
    trigger: On<Remove>,
    // Query<&C, Or<With<ReplicateLike>, (With<Replicate>, With<ReplicationGroup>)>>
    query: Query<(&ReplicationState, Has<NetworkVisibility>, FilteredEntityRef)>,
    registry: Res<ComponentRegistry>,
    root_query: Query<&ReplicateLike>,
    mut manager_query: Query<(Entity, &mut ReplicationSender, &mut MessageManager)>,
    mut pools: ResMut<ClientPools>,
) {
    let entity = trigger.entity;
    let root = root_query.get(entity).map_or(entity, |r| r.root);
    let Ok((replicate, has_network_visibility, entity_ref)) = query.get(root) else {
        return;
    };

    // Note: this is not needed because the SystemParamBuilder already makes sure that the entity
    //  must have both Replicate and Replicating
    // if !entity_ref.contains::<Replicating>() {
    //     return;
    // }

    // only send the removal to senders that have visibility over the entity.
    let senders = replicate.per_sender_state.keys().filter(|sender_entity| {
        replicate
            .per_sender_state
            .get(*sender_entity)
            .is_some_and(|s| s.visibility.is_visible(has_network_visibility))
    });
    manager_query
        .iter_many_unique_mut(senders)
        .for_each(|(sender_entity, mut sender, manager)| {
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
                // Only send removals for components that were previously sent.
                let Some(entity_ticks) = sender.sender_ticks.entities.get_mut(&entity) else {
                    continue;
                };
                let net_id = *registry.kind_map.net_id(kind).unwrap();
                let component_index = net_id as usize;
                if !entity_ticks.components.contains(component_index) {
                    continue;
                }
                trace!(?entity, ?kind, "Sending RemoveComponent");
                entity_ticks.components.remove(component_index);
                sender
                    .pending_actions
                    .add_removals(&mut pools, entity, net_id);
            }
        });
}
