//! General struct handling replication
use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::{
    entity::{EntityHash, EntityIndexMap},
    query::QueryState,
    schedule::IntoScheduleConfigs,
};
use bevy_platform::collections::HashMap;

use crate::components::{ConfirmedTick, InitialReplicated, Replicated};
use crate::registry::registry::{ComponentIndex, ComponentRegistry};
use alloc::vec::Vec;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::RemoteEntityMap;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, trace_span, warn};

use crate::authority::AuthorityBroker;
use crate::error::ReplicationError;
use crate::messages::actions::{ActionFlags, ActionType, ActionsMessage, SpawnAction};
use crate::messages::metadata::SenderMetadata;
use crate::messages::updates::UpdatesMessage;
use crate::plugin::ReplicationSystems;
use crate::prelude::{
    PerSenderReplicationState, Persistent, PreSpawned, ReplicationSender, ReplicationState,
};
use crate::prespawn::PreSpawnedReceiver;
use crate::registry::buffered::{BufferedChanges, BufferedEntity, TempWriteBuffer};
use crate::registry::replication::ReplicationMetadata;
use crate::{plugin, prespawn};
use lightyear_connection::client::{Connected, Disconnected, PeerMetadata};
use lightyear_connection::host::HostClient;
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prelude::{LocalTimeline, Predicted};
use lightyear_core::timeline::NetworkTimeline;
use lightyear_messages::MessageManager;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{MessageReceiver, RemoteEvent};
use lightyear_serde::ToBytes;
use lightyear_serde::reader::{ReadVarInt, Reader};
use lightyear_transport::prelude::Transport;
#[cfg(feature = "metrics")]
use lightyear_utils::metrics::{DormantTimerGauge, TimerGauge};
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[cfg(feature = "client")]
use {lightyear_core::prelude::SyncEvent, lightyear_sync::prelude::client::InputTimelineConfig};

type EntityHashMap<K, V> = HashMap<K, V, EntityHash>;

pub struct ReplicationReceivePlugin;

impl ReplicationReceivePlugin {
    /// On disconnect:
    /// - Despawn any entities that were spawned from replication when the client despawns.
    /// - Reset the ReplicationReceiver to its original state
    fn handle_disconnection(
        trigger: On<Add, Disconnected>,
        mut receiver_query: Query<(&mut ReplicationReceiver, Has<Persistent>)>,
        replicated_query: Query<(Entity, &Replicated), Without<Persistent>>,
        mut commands: Commands,
    ) {
        if let Ok((mut receiver, is_persistent)) = receiver_query.get_mut(trigger.entity) {
            *receiver = ReplicationReceiver::default();
            if is_persistent {
                return;
            }
        };

        // TODO: this should also happen if the ReplicationReceiver is despawned?
        // despawn any entities that were spawned from replication
        replicated_query.iter().for_each(|(entity, replicated)| {
            // TODO: how to avoid this O(n) check? should the replication-receiver maintain a list of received entities?
            if replicated.receiver == trigger.entity
                && let Ok(mut commands) = commands.get_entity(entity)
            {
                commands.despawn();
            }
        });
    }

    // Update the mapping between our local Receiver entity and the remove Sender entity upon receiving the SenderMetadata
    fn receive_sender_metadata(
        trigger: On<RemoteEvent<SenderMetadata>>,
        peer_metadata: Res<PeerMetadata>,
        mut receiver: Query<&mut MessageManager>,
    ) {
        if let Some(receiver_entity) = peer_metadata.mapping.get(&trigger.from)
            && let Ok(mut manager) = receiver.get_mut(*receiver_entity)
        {
            trace!("Add mapping from local Receiver entity to remote Sender entity");
            manager
                .entity_mapper
                .insert(trigger.trigger.sender_entity, *receiver_entity);
        }
    }

    pub(crate) fn apply_world(
        world: &mut World,
        query: &mut QueryState<
            (Entity, &RemoteId),
            (
                With<Connected>,
                With<ReplicationReceiver>,
                With<MessageReceiver<ActionsMessage>>,
                With<MessageReceiver<UpdatesMessage>>,
                With<MessageManager>,
                With<LocalTimeline>,
            ),
        >,
        authority: &mut QueryState<Entity, With<AuthorityBroker>>,
        // buffer to avoid allocations
        mut receiver_entities: Local<Vec<(Entity, PeerId)>>,
    ) {
        #[cfg(feature = "metrics")]
        let _timer = TimerGauge::new("replication/apply");

        // TODO: make sure that the query results are cached! (from iterating archetypes)
        // we first collect the entities we need into a buffer
        // We cannot use query.iter() and &mut World at the same time as this would be UB because they both access Archetypes
        // See https://discord.com/channels/691052431525675048/1358658786851684393/1358793406679355593
        receiver_entities.extend(query.iter(world).map(|(e, remote_id)| (e, remote_id.0)));

        // get the server entity to update the AuthorityBroker state
        let server_entity = authority.single(world).ok();

        // SAFETY: the other uses of `world` won't access the ComponentRegistry
        let unsafe_world = world.as_unsafe_world_cell();
        let component_registry =
            unsafe { unsafe_world.get_resource::<ComponentRegistry>() }.unwrap();
        let world = unsafe { unsafe_world.world_mut() };

        receiver_entities
            .drain(..)
            .for_each(|(entity, remote_peer)| {
                let span = trace_span!("receive", entity = ?entity);
                let _guard = span.enter();
                let unsafe_world = world.as_unsafe_world_cell();

                // TODO: put authority behind feature flag
                // If the receiver also has a ReplicationSender, than we need to handle authority
                // SAFETY: all these accesses don't conflict with the way we use World, which is to spawn new entities
                //  from the replication messages
                let mut entity_mut = unsafe { unsafe_world.world_mut() }.entity_mut(entity);
                let (
                    needs_authority,
                    mut actions_receiver,
                    mut updates_receiver,
                    mut receiver,
                    mut manager,
                    local_timeline,
                ) = unsafe {
                    entity_mut
                        .get_components_mut_unchecked::<(
                            Has<ReplicationSender>,
                            &mut MessageReceiver<ActionsMessage>,
                            &mut MessageReceiver<UpdatesMessage>,
                            &mut ReplicationReceiver,
                            &mut MessageManager,
                            &LocalTimeline,
                        )>()
                        .unwrap()
                };

                // SAFETY: the world will only be used to apply replication updates, which doesn't conflict with other accesses
                let world = unsafe { unsafe_world.world_mut() };

                let tick = local_timeline.tick();
                let mut params = ReceiverParams {
                    receiver_entity: entity,
                    local_tick: tick,
                    remote_peer,
                    receiver: receiver.as_mut(),
                    remote_entity_map: &mut manager.entity_mapper,
                    component_registry,
                    server_entity,
                };

                // TODO: if the message's remote_tick is from the future compared to the
                //  local tick, should we just buffer it and wait until we reach that tick?

                // the Actions channel is sequenced reliable, so we are guaranteed to receive
                // the messages in order. Therefore we should apply them immediately.
                let _ = Self::apply_actions_messages(world, actions_receiver.as_mut(), &mut params)
                    .inspect_err(|e| error!("Could not send actions message: {e:?}"));

                let _ = Self::apply_updates_messages(world, updates_receiver.as_mut(), &mut params)
                    .inspect_err(|e| error!("Could not send updates message: {e:?}"));

                world.flush();
                receiver.tick_cleanup(tick);
            });
    }

    pub(crate) fn apply_actions_messages(
        world: &mut World,
        messages: &mut MessageReceiver<ActionsMessage>,
        params: &mut ReceiverParams,
    ) -> Result<(), ReplicationError> {
        // NOTE: we cannot receive_with_tick because the tick at which the message was sent
        //  might not be the right one because of priority!
        messages.receive().try_for_each(|m| {
            params.receiver.received_this_frame = true;
            let mut reader = Reader::from(m.0);
            let remote_tick = Tick::from_bytes(&mut reader)?;
            params.receiver.last_action_tick = Some(remote_tick);

            let flags = ActionFlags::from_bytes(&mut reader)?;
            let is_last = flags.is_last();
            if flags.has_spawns {
                apply_array(is_last == ActionType::Spawn, &mut reader, |reader| {
                    apply_spawns(world, reader, false, params)
                })?;
            }
            if flags.has_despawns {
                apply_array(is_last == ActionType::Despawn, &mut reader, |reader| {
                    apply_despawns(world, reader, params)
                })?;
            }
            if flags.has_removals {
                apply_array(is_last == ActionType::Removal, &mut reader, |reader| {
                    apply_removals(world, reader, remote_tick, params)
                })?;
            }
            apply_array(true, &mut reader, |reader| {
                apply_updates(world, reader, remote_tick, params, false)
            })?;

            // Flush commands because the entities that were inserted might have triggered some observers
            // In particular, the PreSpawned component triggers an observer that inserts Confirmed, and
            // we want Confirmed to be added so that it can be updated with the correct tick!
            world.flush();
            Ok(())
        })
    }

    pub(crate) fn apply_updates_messages(
        world: &mut World,
        messages: &mut MessageReceiver<UpdatesMessage>,
        params: &mut ReceiverParams,
    ) -> Result<(), ReplicationError> {
        // buffer the updates message
        messages.receive().for_each(|m| {
            // SAFETY: the buffer is guaranteed to be present
            unsafe { params.receiver.updates_buffer.as_mut().unwrap_unchecked() }.insert(m);
        });

        // apply all messages that are ready
        let last_action_tick = params.receiver.last_action_tick;
        // SAFETY: the buffer is guaranteed to be present
        let mut updates_buffer =
            unsafe { params.receiver.updates_buffer.take().unwrap_unchecked() };
        updates_buffer.apply_updates(last_action_tick, |message| {
            let remote_tick = message.remote_tick;
            let mut reader = Reader::from(message.data);
            apply_array(true, &mut reader, |reader| {
                apply_updates(world, reader, remote_tick, params, true)
            })
        })?;

        params.receiver.updates_buffer = Some(updates_buffer);

        Ok(())
    }

    #[cfg(feature = "client")]
    pub(crate) fn on_sync_event(
        trigger: On<SyncEvent<InputTimelineConfig>>,
        mut receiver: Query<&mut ReplicationReceiver>,
    ) {
        receiver.iter_mut().for_each(|mut receiver| {
            // we set `received_this_frame` to true so that we can trigger a rollback check, now that ticks have been updated
            // this is also useful to do a rollback check the first time the InputTimeline is synced, since clients
            // receive replication updates on With<Connected> but do rollback checks on With<IsSynced<InputTimeline>>
            receiver.received_this_frame = true;
            if let Some(tick) = receiver.last_cleanup_tick.as_mut() {
                *tick = *tick + trigger.tick_delta;
            }
            // we don't need to apply the delta to the GroupChannels or to the ConfirmedTick because the ticks stored there are remote ticks
        })
    }
}

impl Plugin for ReplicationReceivePlugin {
    fn build(&self, app: &mut App) {
        // PLUGINS
        if !app.is_plugin_added::<plugin::SharedPlugin>() {
            app.add_plugins(plugin::SharedPlugin);
        }
        if !app.is_plugin_added::<prespawn::PreSpawnedPlugin>() {
            app.add_plugins(prespawn::PreSpawnedPlugin);
        }

        // SYSTEMS
        app.configure_sets(
            PreUpdate,
            ReplicationSystems::Receive.after(MessageSystems::Receive),
        );
        app.add_systems(
            PreUpdate,
            Self::apply_world.in_set(ReplicationSystems::Receive),
        );
        app.add_observer(Self::handle_disconnection);
        app.add_observer(Self::receive_sender_metadata);
        #[cfg(feature = "client")]
        app.add_observer(Self::on_sync_event);
    }
}

#[derive(Debug, Component)]
#[require(Transport)]
pub struct ReplicationReceiver {
    /// Last tick for an action message that was applied.
    /// Can be None if no action messages were applied yet, or if too much time
    /// has passed since the last action message (to avoid tick wrapping issues)
    pub(crate) last_action_tick: Option<Tick>,
    /// Buffer to store updates messages until they are ready to be applied
    /// (An UpdateMessage can only be applied once all action messages that happened
    /// before it have been applied)
    ///
    /// We put it in an Option as we need it to temporarily move it out of the struct
    /// for borrowing reasons.
    pub(crate) updates_buffer: Option<UpdatesBuffer>,
    /// Buffer to apply insertions/removals atomically
    pub(crate) apply_buffer: BufferedChanges,
    /// Tick when we last did a cleanup
    pub(crate) last_cleanup_tick: Option<Tick>,
    #[doc(hidden)]
    /// Flag to indicate if we received a replication message this frame.
    ///
    /// This is only used to know if we should do a rollback check or not.
    /// The flag is set to true when replication messages are received, and reset to false
    /// during the rollback check.
    /// (it's reset to false in the rollback check because we also set it to true when the InputTimeline is synced,
    /// so that on the first sync we also do a rollback check)
    pub received_this_frame: bool,
}

impl Default for ReplicationReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplicationReceiver {
    pub(crate) fn new() -> Self {
        Self {
            last_action_tick: None,
            updates_buffer: Some(UpdatesBuffer::default()),
            apply_buffer: BufferedChanges::default(),
            last_cleanup_tick: None,
            received_this_frame: false,
        }
    }

    // TODO: need to tick cleanup the ConfirmedTicks!
    /// Ticks wrap around u16::max, so if too much time has passed the ticks might become invalid
    /// We handle this by periodically updating the latest_tick for the group
    pub(crate) fn tick_cleanup(&mut self, tick: Tick) {
        // skip cleanup if we did one recently
        if self
            .last_cleanup_tick
            .is_some_and(|last| tick < last + (i16::MAX / 3))
        {
            return;
        }
        self.last_cleanup_tick = Some(tick);

        // TODO: CLEAN UP ALL CONFIRMED TICKS!
        if let Some(last_action_tick) = self.last_action_tick
            && tick - last_action_tick > (i16::MAX / 2)
        {
            self.last_action_tick = None;
        }
    }
}

// TODO: try a sequence buffer?
/// Stores the [`UpdatesMessage`] for a given [`ReplicationGroup`](crate::prelude::ReplicationGroup), sorted
/// in descending remote tick order (the most recent tick first, the oldest tick last)
///
/// The first element is the remote tick, the second is the message
#[derive(Debug)]
pub(crate) struct UpdatesBuffer(Vec<UpdatesMessage>);

impl Default for UpdatesBuffer {
    fn default() -> Self {
        Self(Vec::with_capacity(1))
    }
}
impl UpdatesBuffer {
    fn clear(&mut self) {
        self.0.clear();
    }

    /// Insert a new message in the right position to make sure that the buffer
    /// is still sorted in descending order
    fn insert(&mut self, message: UpdatesMessage) {
        let index = self
            .0
            .partition_point(|m| message.remote_tick < m.remote_tick);
        self.0.insert(index, message);
    }

    /// Number of messages in the buffer
    fn len(&self) -> usize {
        self.0.len()
    }

    /// Get the index of the most recent element in the buffer which has a last_action_tick <= latest_tick,
    /// i.e. the latest_tick that has already been applied to the entity is more recent than the
    /// 'last_action_tick' for that update
    ///
    /// or None if there are None
    fn max_index_to_apply(&self, latest_tick: Option<Tick>) -> Option<usize> {
        // we can use partition point because we know that all the non-ready elements (too recent, we haven't reached their last_action_tick)
        // will be on the left and the ready elements (we have reached their last_action_tick) will be on the right.
        // Returning false means that the element is ready to be applied
        let idx = self.0.partition_point(|message| {
            let Some(last_action_tick) = message.last_action_tick else {
                // if the Updates message had no last_action_tick constraint (for example
                // because the authority got swapped so the first message sent is an Update, not an Action),
                // then we can apply it immediately!
                return false;
            };
            let Some(latest_tick) = latest_tick else {
                // if the Updates message requires a certain last_action_tick to be applied
                // and locally we haven't applied any actions yet, we can't apply it!
                return true;
            };
            last_action_tick > latest_tick
        });
        if idx == self.len() { None } else { Some(idx) }
    }

    /// Pop the oldest tick from the buffer
    fn pop_oldest(&mut self) -> Option<UpdatesMessage> {
        self.0.pop()
    }

    /// Apply a function `f` to all [`UpdatesMessage`] that are ready.
    ///
    /// (All actions that happened before that update have been received)
    fn apply_updates(
        &mut self,
        last_action_tick: Option<Tick>,
        mut f: impl FnMut(UpdatesMessage) -> Result<(), ReplicationError>,
    ) -> Result<(), ReplicationError> {
        // the buffered_channel is sorted in descending order,
        // [most_recent_tick, ...,  max_readable_tick (based on last_action_tick), ..., oldest_tick]
        // What we want is to return (not necessarily in order) [max_readable_tick, ..., oldest_tick]
        // along with a flag that lets us know if we are the max_readable_tick or not.
        // (max_readable_tick is the only one we want to actually apply to the world, because the other
        //  older updates are redundant. The older ticks are included so that we can have a comprehensive
        //  confirmed history, for example to have a better interpolation)
        let Some(max_applicable_idx) = self.max_index_to_apply(last_action_tick) else {
            return Ok(());
        };
        while self.len() > max_applicable_idx {
            let message = self.pop_oldest().unwrap();
            f(message)?;
        }
        Ok(())
    }
}

pub struct ReceiverParams<'a> {
    receiver_entity: Entity,
    local_tick: Tick,
    remote_peer: PeerId,
    receiver: &'a mut ReplicationReceiver,
    remote_entity_map: &'a mut RemoteEntityMap,
    component_registry: &'a ComponentRegistry,
    server_entity: Option<Entity>,
}

/// Read a sequence of items from the reader, applying `f` to each item.
fn apply_array(
    is_last: bool,
    reader: &mut Reader,
    mut f: impl FnMut(&mut Reader) -> Result<(), ReplicationError>,
) -> Result<(), ReplicationError> {
    // for the last array in the message, we can just read until the end of the buffer
    if is_last {
        while reader.has_remaining() {
            f(reader)?;
        }
    } else {
        let len = reader.read_varint()? as usize;
        for _ in 0..len {
            f(reader)?;
        }
    }
    Ok(())
}

/// Apply spawns
fn apply_spawns(
    world: &mut World,
    mut reader: &mut Reader,
    needs_authority: bool,
    params: &mut ReceiverParams<'_>,
) -> Result<(), ReplicationError> {
    let remote_entity = Entity::from_bytes(reader)?;
    let remote_tick = Tick::from_bytes(reader)?;
    let spawn_action = SpawnAction::from_bytes(&mut reader)?;

    let insert_sync_components = |#[cfg(feature = "prediction")] predicted: bool,
                                  #[cfg(feature = "interpolation")] interpolated: bool,
                                  entity: &mut EntityWorldMut,
                                  remote_tick: Tick| {
        #[cfg(any(feature = "interpolation", feature = "prediction"))]
        if interpolated || predicted {
            entity.insert(ConfirmedTick { tick: remote_tick });
        }
        #[cfg(feature = "interpolation")]
        if interpolated {
            trace!("Inserting interpolated on local entity {:?}", entity.id());
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("interpolated::spawn").increment(1);
            }
            entity.insert(Interpolated);
        }
        #[cfg(feature = "prediction")]
        if predicted {
            trace!("Inserting predicted on local entity {:?}", entity.id());
            // this might also count PreSpawned entities, even if they ended up not being matched
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("prediction::spawn").increment(1);
            }

            entity.insert(Predicted);
        }
    };

    let world_clone = world.as_unsafe_world_cell();
    let mut prespawned_receiver =
        unsafe { world_clone.world_mut() }.get_mut::<PreSpawnedReceiver>(params.receiver_entity);
    let mut authority_broker = params
        .server_entity
        .and_then(|e| unsafe { world_clone.world_mut() }.get_mut::<AuthorityBroker>(e));
    // SAFETY: the rest of the function won't use world to access PreSpawnedReceiver or AuthorityBroker
    let world = unsafe { world_clone.world_mut() };

    // check if the entity can already be mapped to an existing local entity.
    // This can happen with authority transfer or prespawning
    // (e.g client spawned an entity and then transfer the authority to the server.
    //  The server will then send a spawn message)
    if let Some(local_entity) = spawn_action
        .prespawn
        .and_then(|hash| {
            prespawned_receiver
                .as_mut()
                .and_then(|receiver| receiver.matches(hash, remote_entity))
        })
        .inspect(|e| {
            debug!(?remote_entity, local_entity = ?e, "Update prespawn entity map");
            // we update the entity map for the prespawning case
            params.remote_entity_map.insert(remote_entity, *e);
        })
        .or(params.remote_entity_map.get_local(remote_entity))
    {
        // if we received the entity from the remote, then we don't have authority over it
        if let Some(ref mut broker) = authority_broker {
            broker
                .owners
                .entry(local_entity)
                .or_insert(Some(params.remote_peer));
        }
        if let Ok(mut local_entity) = world.get_entity_mut(local_entity) {
            debug!(
                "Received spawn for entity {:?} that already exists. This might be because of an authority transfer or prespawning.",
                local_entity.id()
            );
            // if the entity is predicted, we remove PreSpawned no matter if there is a match
            // - if match, the entity is now predicted and we want to rollback to the Confirmed<C> state
            //   (while PreSpawned, we rollback to the value of the component in the history)
            // we also want to remove PreSpawned for inputs entities because we want to
            // send InputMessages using the Entity instead of the hash
            local_entity.remove::<PreSpawned>();

            insert_sync_components(
                #[cfg(feature = "prediction")]
                spawn_action.predicted,
                #[cfg(feature = "interpolation")]
                spawn_action.interpolated,
                &mut local_entity,
                remote_tick,
            );
            return Ok(());
        }
        // TODO: if this is prespawned, the prespawned entity could already have been despawned! add metrics/logs
        // #[cfg(feature = "metrics")]
        // {
        //     metrics::counter!("prespawn::match::missing").increment(1);
        // }

        warn!(
            "Received spawn for an entity that is already in our entity mapping but doesn't exist! Not spawning"
        );
        return Ok(());
    }

    // NOTE: at this point we know that the remote entity was not mapped!
    let mut local_entity = world.spawn((
        Replicated {
            receiver: params.receiver_entity,
        },
        InitialReplicated {
            receiver: params.receiver_entity,
        },
        ConfirmedTick { tick: remote_tick },
    ));

    // if we received the entity from the remote, then we don't have authority over it
    if let Some(ref mut broker) = authority_broker {
        broker
            .owners
            .insert(local_entity.id(), Some(params.remote_peer));
    }
    if needs_authority {
        local_entity.insert(ReplicationState {
            per_sender_state: EntityIndexMap::from([(
                params.receiver_entity,
                PerSenderReplicationState::without_authority(),
            )]),
        });
    }

    debug!(
        "Received entity spawn for remote entity {remote_entity:?}. Spawned local entity {:?}",
        local_entity.id()
    );
    insert_sync_components(
        #[cfg(feature = "prediction")]
        spawn_action.predicted,
        #[cfg(feature = "interpolation")]
        spawn_action.interpolated,
        &mut local_entity,
        remote_tick,
    );

    params
        .remote_entity_map
        .insert(remote_entity, local_entity.id());
    trace!("Updated remote entity map: {:?}", params.remote_entity_map);
    Ok(())
}

fn apply_despawns(
    world: &mut World,
    reader: &mut Reader,
    params: &mut ReceiverParams<'_>,
) -> Result<(), ReplicationError> {
    let remote_entity = Entity::from_bytes(reader)?;
    if let Some(local_entity) = params.remote_entity_map.remove_by_remote(remote_entity) {
        if let Ok(entity_mut) = world.get_entity_mut(local_entity) {
            entity_mut.despawn();
        }
    } else {
        error!("Received despawn for an entity that does not exist")
    }
    Ok(())
}

fn update_confirmed_tick(
    local_entity: Entity,
    mut confirmed: Mut<ConfirmedTick>,
    remote_tick: Tick,
) {
    trace!(
        ?remote_tick,
        ?local_entity,
        "updating confirmed tick for entity"
    );
    confirmed.tick = remote_tick;
}

fn apply_removals(
    world: &mut World,
    mut reader: &mut Reader,
    remote_tick: Tick,
    params: &mut ReceiverParams<'_>,
) -> Result<(), ReplicationError> {
    let remote_entity = Entity::from_bytes(reader)?;
    let Some(mut local_entity_mut) = params.remote_entity_map.get_by_remote(world, remote_entity)
    else {
        error!(?remote_entity, "cannot find entity");
        return Ok(());
    };
    // SAFETY: these components don't alias
    let Some((confirmed, predicted, interpolated)) = unsafe {
        local_entity_mut.get_components_mut_unchecked::<(&mut ConfirmedTick, Has<Predicted>, Has<Interpolated>)>()
    };

    let local_entity = local_entity_mut.id();
    update_confirmed_tick(local_entity, confirmed, remote_tick);
    let num_removals = reader.read_varint()? as usize;
    let mut buffered_entity = BufferedEntity {
        entity: local_entity_mut,
        buffered: &mut params.receiver.apply_buffer,
    };
    for _ in 0..num_removals {
        let component_net_id = ComponentIndex::from_bytes(&mut reader)?;
        params.component_registry.remove(
            component_net_id,
            &mut buffered_entity,
            predicted,
            interpolated,
            remote_tick,
        );
    }
    buffered_entity.apply();
    Ok(())
}

fn apply_updates(
    world: &mut World,
    mut reader: &mut Reader,
    remote_tick: Tick,
    params: &mut ReceiverParams<'_>,
    // True if this comes from an update message (as opposed to an action message)
    is_update: bool,
) -> Result<(), ReplicationError> {
    let remote_entity = Entity::from_bytes(reader)?;
    let Some(mut local_entity_mut) = params.remote_entity_map.get_by_remote(world, remote_entity)
    else {
        error!(?remote_entity, "cannot find entity");
        return Ok(());
    };

    // SAFETY: these components don't alias
    let Some((confirmed, predicted, interpolated)) = unsafe {
        local_entity_mut.get_components_mut_unchecked::<(&mut ConfirmedTick, Has<Predicted>, Has<Interpolated>)>()
    };
    let local_entity = local_entity_mut.id();

    // TODO: handle authority
    // the local Sender has authority over the entity, so we don't want to accept the updates
    if local_entity_mut
        .get::<ReplicationState>()
        .as_ref()
        .is_some_and(|s| s.has_authority(params.receiver_entity))
    {
        trace!(
            "Ignored a replication action received from peer {:?} since the receiver has authority over the entity: {:?}",
            params.remote_peer, local_entity
        );
        return Ok(());
    }

    // ignore old updates
    // TODO: this needs tick-wrapping! or just use u32 for ticks
    if is_update && remote_tick < confirmed.tick {
        return Ok(());
    }

    update_confirmed_tick(local_entity, confirmed, remote_tick);
    let num_updates = reader.read_varint()? as usize;
    let mut buffered_entity = BufferedEntity {
        entity: local_entity_mut,
        buffered: &mut params.receiver.apply_buffer,
    };
    for _ in 0..num_updates {
        params.component_registry.buffer(
            reader,
            &mut buffered_entity,
            remote_tick,
            &mut params.remote_entity_map.remote_to_local,
            predicted,
            interpolated,
        )?;
    }
    buffered_entity.apply();
    Ok(())
}
