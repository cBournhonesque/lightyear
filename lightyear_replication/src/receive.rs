//! General struct handling replication
use alloc::collections::BTreeMap;
use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::{
    entity::{EntityHash, EntityIndexMap},
    query::QueryState,
    schedule::IntoScheduleConfigs,
};
use bevy_platform::collections::{HashMap, HashSet};

use crate::components::{ConfirmedTick, InitialReplicated, Replicated};
use crate::message::{ActionsMessage, SenderMetadata, SpawnAction, UpdatesMessage};
use crate::registry::registry::ComponentRegistry;
use alloc::vec::Vec;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::RemoteEntityMap;
use lightyear_transport::packet::message::MessageId;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, trace_span, warn};

use crate::authority::AuthorityBroker;
use crate::plugin::ReplicationSystems;
use crate::prelude::{
    PerSenderReplicationState, Persistent, PreSpawned, ReplicationGroupId, ReplicationSender,
    ReplicationState,
};
use crate::prespawn::PreSpawnedReceiver;
use crate::registry::buffered::{BufferedChanges, BufferedEntity};
use crate::{plugin, prespawn};
use lightyear_connection::client::{Connected, Disconnected, PeerMetadata};
use lightyear_connection::host::HostClient;
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prelude::{LocalTimeline, Predicted};
use lightyear_messages::MessageManager;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{MessageReceiver, RemoteEvent};
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

    pub(crate) fn receive_messages(
        mut query: Query<
            (
                &mut MessageReceiver<ActionsMessage>,
                &mut MessageReceiver<UpdatesMessage>,
                &mut ReplicationReceiver,
            ),
            // On the Host-Client there is no replication messages to receive since the entities
            // from the sender are in the same world!
            (With<Connected>, Without<HostClient>),
        >,
    ) {
        #[cfg(feature = "metrics")]
        let _timer = DormantTimerGauge::new("replication/receive");

        query
            .par_iter_mut()
            .for_each(|(mut actions, mut updates, mut receiver)| {
                for message in actions.receive_with_tick() {
                    receiver.recv_actions(message.data, message.remote_tick);
                }
                for message in updates.receive_with_tick() {
                    receiver.recv_updates(message.data, message.remote_tick);
                }
                #[cfg(feature = "metrics")]
                _timer.activate();
            });
    }

    pub(crate) fn apply_world(
        world: &mut World,
        query: &mut QueryState<
            (Entity, &RemoteId),
            (
                With<Connected>,
                With<ReplicationReceiver>,
                With<MessageManager>,
            ),
        >,
        authority: &mut QueryState<Entity, With<AuthorityBroker>>,
        // buffer to avoid allocations
        mut receiver_entities: Local<Vec<(Entity, PeerId)>>,
    ) {
        #[cfg(feature = "metrics")]
        let _timer = TimerGauge::new("replication/apply");

        let tick = world.resource::<LocalTimeline>().tick();

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
                let (needs_authority, mut receiver, mut manager) = unsafe {
                    entity_mut
                        .get_components_mut_unchecked::<(
                            Has<ReplicationSender>,
                            &mut ReplicationReceiver,
                            &mut MessageManager,
                        )>()
                        .unwrap()
                };

                // SAFETY: the world will only be used to apply replication updates, which doesn't conflict with other accesses
                let world = unsafe { unsafe_world.world_mut() };

                receiver.apply_world(
                    world,
                    entity,
                    remote_peer,
                    &mut manager.entity_mapper,
                    needs_authority,
                    server_entity,
                    component_registry,
                    tick,
                );
                receiver.tick_cleanup(tick);
            });
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
            (Self::receive_messages, Self::apply_world)
                .chain()
                .in_set(ReplicationSystems::Receive),
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
    pub(crate) buffer: BufferedChanges,
    /// Buffer to so that we have an ordered receiver per group
    pub(crate) group_channels: EntityHashMap<ReplicationGroupId, GroupChannel>,

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
            buffer: BufferedChanges::default(),
            group_channels: Default::default(),
            last_cleanup_tick: None,
            received_this_frame: false,
        }
    }

    /// Buffer a received [`ActionsMessage`].
    ///
    /// The remote_tick is the tick at which the message was buffered and sent by the remote client.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn recv_actions(&mut self, actions: ActionsMessage, remote_tick: Tick) {
        trace!(
            ?actions,
            ?remote_tick,
            "Received ReplicationActions message"
        );
        let channel = self.group_channels.entry(actions.group_id).or_default();

        // if the message is too old, ignore it
        if actions.sequence_id < channel.actions_pending_recv_message_id {
            trace!(message_id= ?actions.sequence_id, pending_message_id = ?channel.actions_pending_recv_message_id, "message is too old, ignored");
            return;
        }
        self.received_this_frame = true;

        // add the message to the buffer
        // TODO: I guess this handles potential duplicates?
        channel
            .actions_recv_message_buffer
            .insert(actions.sequence_id, (remote_tick, actions));
        trace!(?channel, "group channel after buffering");
    }

    /// Buffer a received [`UpdatesMessage`].
    ///
    /// The remote_tick is the tick at which the message was buffered and sent by the remote client.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn recv_updates(&mut self, updates: UpdatesMessage, remote_tick: Tick) {
        // TODO: instead of storing the group_id, we could do entity_mapping for the first entity and get group_channel from there?
        trace!(?updates, ?remote_tick, "Received replication message");
        let channel = self.group_channels.entry(updates.group_id).or_default();

        // NOTE: this is valid even after tick wrapping because we keep clamping the latest_tick values for each channel
        // if we have already applied a more recent update for this group, no need to keep this one (or should we keep it for history?)
        if channel.latest_tick.is_some_and(|t| remote_tick <= t) {
            trace!(
                "discard because the update's tick {remote_tick:?} is older than the latest tick {:?}",
                channel.latest_tick
            );
            return;
        }

        self.received_this_frame = true;

        // TODO: what we want is
        //  - if the update is for a tick in the past compared to our local state, we can safely ignore immediately
        //  - make sure that the local state has a `latest_tick` that is bigger than the update's remote tick (i.e.
        //  we only apply remote ticks if we have reached the last_action_tick for that update)
        //  - if we have two updates that satisfy those conditions, we don't need to buffer both!
        //   We can just keep the one with the biggest last_action_tick? since eventually that's the only one we're going to apply.
        //   Possible exceptions:
        //   - we want to keep all the intermediary information to put it in a history for interpolation (so that instead of interpolating
        //     only between the updates we apply that have the highest tick, we interpolate between all updates received. The interpolation
        //     tick could be much further in the past. Or maybe check the interpolation tick?)
        //   - we could be delaying some intermediary updates because the update with higher tick also has a higher last_action_tick,
        //     and we might have some intermediary updates that we could be applying.
        //     For example `latest_tick` is 5, we receive an update from tick 20 with last_action_tick = 15, and we receive an update
        //     from tick 10 with last_action tick = 7. Even If we receive the action_tick for tick 7, we wouldn't be able to apply it right away
        //     because we're waiting for the action_tick for tick 15. So we should keep both updates, and apply them as soon as possible (as soon
        //     as the smallest last_action_tick is reached)
        //     However in practice this seems expensive to do, and a rare case. For now, let's just only keep the update with the highest tick?
        //     TODO: check that this is correct even with delta_compression.

        // TODO: could we use a FreeList here? (SequenceBuffer?) Updates are only buffered until we reach their last_action_tick
        //  which should be fairly quick, never more than 1-2 sec. (so a buffer of size 64 or 128 seems good). It might need more memory though?
        //  Benchmark.
        channel.buffered_updates.insert(updates, remote_tick);

        // TODO: include somewhere in the update message the m.last_ack_tick since when we compute changes?
        //  (if we want to do diff compression?)
        trace!(?channel, "group channel after buffering");
    }

    /// Ticks wrap around u32::max, so if too much time has passed the ticks might become invalid
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
        // if it's been enough time since we last had any update for the group, we update the latest_tick for the group
        for group_channel in self.group_channels.values_mut() {
            if let Some(latest_tick) = group_channel.latest_tick
                && tick - latest_tick > (i16::MAX / 2)
            {
                debug!(
                    ?group_channel,
                    "Setting the latest_tick {latest_tick:?} to tick {tick:?} because there hasn't been any new updates in a while"
                );
                group_channel.latest_tick = Some(tick);
            }
        }
    }
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
///
/// - all component inserts/removes/updates for an entity to be grouped together in a single message
impl ReplicationReceiver {
    // TODO: how can I emit metrics here that contain the channel kind?
    //  use a OnceCell that gets set with the channel name mapping when the protocol is finalized?
    //  the other option is to have wrappers in Connection, but that's pretty ugly

    /// Read from the buffer the EntityActionsMessage and EntityUpdatesMessage that are ready,
    /// and apply them to the World
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_world(
        &mut self,
        world: &mut World,
        receiver_entity: Entity,
        remote: PeerId,
        remote_entity_map: &mut RemoteEntityMap,
        needs_authority: bool,
        server_entity: Option<Entity>,
        component_registry: &ComponentRegistry,
        current_tick: Tick,
    ) {
        // apply all actions first
        self.group_channels
            .iter_mut()
            .for_each(|(group_id, channel)| {
                loop {
                    let Some((remote_tick, _)) = channel
                        .actions_recv_message_buffer
                        .get(&channel.actions_pending_recv_message_id)
                    else {
                        return;
                    };
                    // TODO: should we store the message in a buffer if it's in the future,
                    //  and only apply it at the correct tick?
                    // // if the message is from the future, keep it there
                    // if *remote_tick > current_tick {
                    //     debug!(
                    //         "message tick {:?} is from the future compared to our current tick {:?}",
                    //         remote_tick, current_tick
                    //     );
                    //     return;
                    // }

                    // We have received the message we are waiting for
                    let (remote_tick, message) = channel
                        .actions_recv_message_buffer
                        .remove(&channel.actions_pending_recv_message_id)
                        .unwrap();

                    channel.actions_pending_recv_message_id += 1;
                    // Update the latest server tick that we have processed
                    channel.latest_tick = Some(remote_tick);

                    channel.apply_actions_message(
                        world,
                        receiver_entity,
                        remote,
                        component_registry,
                        remote_tick,
                        message,
                        remote_entity_map,
                        needs_authority,
                        server_entity,
                        &mut self.buffer,
                    );
                }
            });

        self.group_channels
            .iter_mut()
            .for_each(|(group_id, channel)| {
                // the buffered_channel is sorted in descending order,
                // [most_recent_tick, ...,  max_readable_tick (based on last_action_tick), ..., oldest_tick]
                // What we want is to return (not necessarily in order) [max_readable_tick, ..., oldest_tick]
                // along with a flag that lets us know if we are the max_readable_tick or not.
                // (max_readable_tick is the only one we want to actually apply to the world, because the other
                //  older updates are redundant. The older ticks are included so that we can have a comprehensive
                //  confirmed history, for example to have a better interpolation)
                // Any tick more recent than `max_readable_tick` cannot be applied yet, because they have a 'last_action_tick'
                //  that hasn't been applied to the receiver's world
                let Some(max_applicable_idx) = channel
                    .buffered_updates
                    .max_index_to_apply(channel.latest_tick)
                else {
                    return;
                };

                // pop the oldest until we reach the max applicable index
                while channel.buffered_updates.len() > max_applicable_idx {
                    let (remote_tick, message) = channel.buffered_updates.pop_oldest().unwrap();

                    // We restricted the updates only to those that have a last_action_tick <= latest_tick,
                    // but we also need to make sure that we don't apply updates that are too old!
                    // (older than the latest_tick applied from any Actions message above!)
                    //
                    // Note that the channel.latest tick could still be None in case of authority-transfer!
                    if channel
                        .latest_tick
                        .is_some_and(|latest_tick| remote_tick <= latest_tick)
                    {
                        // TODO: those ticks could be history and could be interesting. They are older than the latest_tick though
                        continue;
                    }

                    // These ticks are more recent than the latest_tick, but only the most recent one is interesting to us
                    let is_history = channel.buffered_updates.len() != max_applicable_idx;
                    // most recent tick.
                    if !is_history {
                        // TODO: maybe instead of relying on this we could update the Confirmed.tick via event
                        //  after PredictionSet::Spawn?
                        // it is important to update the `latest_tick` because it is used to populate
                        // the Confirmed.tick when the Confirmed entity is just spawned
                        channel.latest_tick = Some(remote_tick);
                    }
                    channel.apply_updates_message(
                        world,
                        receiver_entity,
                        remote,
                        component_registry,
                        remote_tick,
                        is_history,
                        message,
                        remote_entity_map,
                    );
                }
            })
    }
}

/// Channel to keep track of receiving/sending replication messages for a given Group
#[derive(Debug)]
pub struct GroupChannel {
    // entities
    // set of local entities that are part of the same Replication Group
    // (we use local entities because we might not be aware of the remote entities,
    //  if the remote is doing pre-mapping)
    pub(crate) local_entities: HashSet<Entity>,
    // actions
    pub(crate) actions_pending_recv_message_id: MessageId,
    pub(crate) actions_recv_message_buffer: BTreeMap<MessageId, (Tick, ActionsMessage)>,
    // updates
    pub(crate) buffered_updates: UpdatesBuffer,
    /// remote tick of the latest update/action that we applied to the local group
    pub latest_tick: Option<Tick>,
}

impl Default for GroupChannel {
    fn default() -> Self {
        Self {
            local_entities: HashSet::default(),
            actions_pending_recv_message_id: MessageId(0),
            actions_recv_message_buffer: BTreeMap::new(),
            buffered_updates: UpdatesBuffer::default(),
            latest_tick: None,
        }
    }
}

/// Iterator that returns all the available EntityActions for the current [`GroupChannel`]
///
/// Reads a message from the internal buffer to get its content
/// Since we are receiving messages in order, we don't return from the buffer
/// until we have received the message we are waiting for (the next expected MessageId)
/// This assumes that the sender sends all message ids sequentially.
///
/// If had received updates that were waiting on a given action, we also return them
struct ActionsIterator<'a> {
    channel: &'a mut GroupChannel,
    current_tick: Tick,
}

impl Iterator for ActionsIterator<'_> {
    /// The message along with the tick at which the remote message was sent
    type Item = (Tick, ActionsMessage);

    fn next(&mut self) -> Option<Self::Item> {
        // TODO: maybe only get the message if our local client tick is >= to it? (so that we don't apply an update from the future)
        let message = self
            .channel
            .actions_recv_message_buffer
            .get(&self.channel.actions_pending_recv_message_id)?;
        // if the message is from the future, keep it there
        if message.0 > self.current_tick {
            debug!(
                "message tick {:?} is from the future compared to our current tick {:?}",
                message.0, self.current_tick
            );
            return None;
        }

        // We have received the message we are waiting for
        let message = self
            .channel
            .actions_recv_message_buffer
            .remove(&self.channel.actions_pending_recv_message_id)
            .unwrap();

        self.channel.actions_pending_recv_message_id += 1;
        // Update the latest server tick that we have processed
        self.channel.latest_tick = Some(message.0);
        Some(message)
    }
}

// TODO: try a sequence buffer?
/// Stores the [`UpdatesMessage`] for a given [`ReplicationGroup`](crate::prelude::ReplicationGroup), sorted
/// in descending remote tick order (the most recent tick first, the oldest tick last)
///
/// The first element is the remote tick, the second is the message
#[derive(Debug)]
pub(crate) struct UpdatesBuffer(Vec<(Tick, UpdatesMessage)>);

/// Update that is given to `apply_world`
#[derive(Debug, PartialEq)]
struct Update {
    remote_tick: Tick,
    message: UpdatesMessage,
    /// If true, we don't want to apply the update to the world, because we are going
    /// to apply a more recent one
    is_history: bool,
}
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
    fn insert(&mut self, message: UpdatesMessage, remote_tick: Tick) {
        let index = self.0.partition_point(|(tick, _)| remote_tick < *tick);
        self.0.insert(index, (remote_tick, message));
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
        let idx = self.0.partition_point(|(_, message)| {
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
    fn pop_oldest(&mut self) -> Option<(Tick, UpdatesMessage)> {
        self.0.pop()
    }
}

/// Iterator that returns all the available [`UpdatesMessage`] for the current [`GroupChannel`]
///
/// We read from the [`UpdatesBuffer`] in ascending remote tick order:
/// - if we have not reached the last_action_tick for a given update, we stop there
/// - else, we return all the updates whose last_action_tick is reached, and
struct UpdatesIterator<'a> {
    channel: &'a mut GroupChannel,
    /// We iterate until we reach this idx in the buffer
    max_applicable_idx: Option<usize>,
}

impl Iterator for UpdatesIterator<'_> {
    /// The message along with the tick at which the remote message was sent
    type Item = Update;

    fn next(&mut self) -> Option<Self::Item> {
        // TODO: NEED TO REIMPLEMENT THIS TODO!
        // TODO: maybe only get the message if our local client tick is >= to it? (so that we don't apply an update from the future)

        // TODO: ideally we do this update only once, when instantiating the iterator?
        // if we cannot apply any updates, return None
        let max_applicable_idx = self.max_applicable_idx?;

        // we have returned all the items that were ready, stop now
        if self.channel.buffered_updates.len() == max_applicable_idx {
            return None;
        }

        // pop the oldest until we reach the max applicable index
        let (remote_tick, message) = self.channel.buffered_updates.pop_oldest().unwrap();
        let is_history = self.channel.buffered_updates.len() != max_applicable_idx;
        if !is_history {
            // TODO: maybe instead of relying on this we could update the Confirmed.tick via event
            //  after PredictionSet::Spawn?
            // it is important to update the `latest_tick` because it is used to populate
            // the Confirmed.tick when the Confirmed entity is just spawned
            self.channel.latest_tick = Some(remote_tick);
        }
        Some(Update {
            remote_tick,
            message,
            is_history,
        })
    }
}

impl GroupChannel {
    /// Builds an iterator that returns all the available EntityActions for the current [`GroupChannel`]
    fn read_actions(&mut self, current_tick: Tick) -> ActionsIterator<'_> {
        ActionsIterator {
            channel: self,
            current_tick,
        }
    }

    /// Builds an iterator that returns all the available EntityUpdates for the current [`GroupChannel`]
    /// Needs to run after read_actions for correctness (because we need to update the `latest_tick` of
    /// the group before we can apply the updates)
    fn read_updates(&mut self) -> UpdatesIterator<'_> {
        // the buffered_channel is sorted in descending order,
        // [most_recent_tick, ...,  max_readable_tick (based on last_action_tick), ..., oldest_tick]
        // What we want is to return (not necessarily in order) [max_readable_tick, ..., oldest_tick]
        // along with a flag that lets us know if we are the max_readable_tick or not.
        // (max_readable_tick is the only one we want to actually apply to the world, because the other
        //  older updates are redundant. The older ticks are included so that we can have a comprehensive
        //  confirmed history, for example to have a better interpolation)
        let max_applicable_idx = self.buffered_updates.max_index_to_apply(self.latest_tick);

        UpdatesIterator {
            channel: self,
            max_applicable_idx,
        }
    }

    /// Apply actions for channel
    pub(crate) fn apply_actions_message(
        &mut self,
        world: &mut World,
        receiver_entity: Entity,
        remote: PeerId,
        component_registry: &ComponentRegistry,
        remote_tick: Tick,
        message: ActionsMessage,
        remote_entity_map: &mut RemoteEntityMap,
        needs_authority: bool,
        server_entity: Option<Entity>,
        temp_write_buffer: &mut BufferedChanges,
    ) {
        let insert_sync_components = |#[cfg(feature = "prediction")] predicted: bool,
                                      #[cfg(feature = "interpolation")] interpolated: bool,
                                      entity: &mut EntityWorldMut,
                                      remote_tick: Tick| {
            #[cfg(any(feature = "interpolation", feature = "prediction"))]
            let mut add_confirmed = false;
            #[cfg(feature = "interpolation")]
            if interpolated {
                add_confirmed = true;
            }
            #[cfg(feature = "prediction")]
            if predicted {
                add_confirmed = true;
            }
            #[cfg(any(feature = "interpolation", feature = "prediction"))]
            if add_confirmed {
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

        let group_id = message.group_id;
        // for the default group, there is no guarantee that the updates for all entities in the group
        // is received at the same time
        let is_default_group = group_id == ReplicationGroupId(0);
        debug!(
            ?remote_tick,
            ?message,
            "Received replication actions from remote: {remote:?}"
        );
        let world_clone = world.as_unsafe_world_cell();
        let mut prespawned_receiver =
            unsafe { world_clone.world_mut() }.get_mut::<PreSpawnedReceiver>(receiver_entity);
        let mut authority_broker = server_entity
            .and_then(|e| unsafe { world_clone.world_mut() }.get_mut::<AuthorityBroker>(e));
        // SAFETY: the rest of the function won't use world to access PreSpawnedReceiver or AuthorityBroker
        let world = unsafe { world_clone.world_mut() };

        // NOTE: order matters here, because some components can depend on other entities.
        // These components could even form a cycle, for example A.HasWeapon(B) and B.HasHolder(A)
        // Our solution is to first handle spawn for all entities separately.
        for (remote_entity, actions) in message.into_iter() {
            // spawn
            if let SpawnAction::Spawn {
                #[cfg(feature = "prediction")]
                predicted,
                #[cfg(feature = "interpolation")]
                interpolated,
                prespawn,
            } = actions.spawn
            {
                // check if the entity can already be mapped to an existing local entity.
                // This can happen with authority transfer or prespawning
                // (e.g client spawned an entity and then transfer the authority to the server.
                //  The server will then send a spawn message)
                if let Some(local_entity) = prespawn
                    .and_then(|hash| {
                        prespawned_receiver
                            .as_mut()
                            .and_then(|receiver| receiver.matches(hash, remote_entity))
                    })
                    .inspect(|e| {
                        debug!(?remote_entity, local_entity = ?e, "Update prespawn entity map");
                        // we update the entity map for the prespawning case
                        remote_entity_map.insert(remote_entity, *e);
                    })
                    .or(remote_entity_map.get_local(remote_entity))
                {
                    // if we received the entity from the remote, then we don't have authority over it
                    if let Some(ref mut broker) = authority_broker {
                        broker.owners.entry(local_entity).or_insert(Some(remote));
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
                            predicted,
                            #[cfg(feature = "interpolation")]
                            interpolated,
                            &mut local_entity,
                            remote_tick,
                        );
                        // we still need to update the local entity to group mapping on the receiver
                        if !is_default_group {
                            self.local_entities.insert(local_entity.id());
                        }
                        continue;
                    }
                    // TODO: if this is prespawned, the prespawned entity could already have been despawned! add metrics/logs
                    // #[cfg(feature = "metrics")]
                    // {
                    //     metrics::counter!("prespawn::match::missing").increment(1);
                    // }

                    warn!(
                        "Received spawn for an entity that is already in our entity mapping but doesn't exist! Not spawning"
                    );
                    continue;
                }

                // NOTE: at this point we know that the remote entity was not mapped!
                let mut local_entity = world.spawn((
                    Replicated {
                        receiver: receiver_entity,
                    },
                    InitialReplicated {
                        receiver: receiver_entity,
                    },
                ));
                // if we received the entity from the remote, then we don't have authority over it
                if let Some(ref mut broker) = authority_broker {
                    broker.owners.insert(local_entity.id(), Some(remote));
                }
                if needs_authority {
                    local_entity.insert(ReplicationState {
                        per_sender_state: EntityIndexMap::from([(
                            receiver_entity,
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
                    predicted,
                    #[cfg(feature = "interpolation")]
                    interpolated,
                    &mut local_entity,
                    remote_tick,
                );

                if !is_default_group {
                    self.local_entities.insert(local_entity.id());
                }
                remote_entity_map.insert(remote_entity, local_entity.id());
                trace!("Updated remote entity map: {:?}", remote_entity_map);
            }
        }

        for (entity, actions) in message.into_iter() {
            // despawn
            if actions.spawn == SpawnAction::Despawn {
                if let Some(local_entity) = remote_entity_map.remove_by_remote(entity) {
                    if !is_default_group {
                        self.local_entities.remove(&local_entity);
                    }
                    // TODO: we despawn all children as well right now, but that might not be what we want?
                    if let Ok(entity_mut) = world.get_entity_mut(local_entity) {
                        entity_mut.despawn();
                    }
                } else {
                    error!("Received despawn for an entity that does not exist")
                }
                continue;
            }

            // safety: we know by this point that the entity exists
            let Some(mut local_entity_mut) = remote_entity_map.get_by_remote(world, entity) else {
                error!(?entity, "cannot find entity");
                continue;
            };
            let local_entity = local_entity_mut.id();

            // for the default group: update the ConfirmedTick now
            if is_default_group
                && let Some(mut confirmed) = local_entity_mut.get_mut::<ConfirmedTick>()
            {
                trace!(
                    ?remote_tick,
                    ?local_entity,
                    "updating confirmed tick for entity"
                );
                confirmed.tick = remote_tick;
            }

            // check if the entity is predicted or interpolated, in which case we want to replicate C as Confirmed<C>
            // (C will be the Predicted or Interpolated value)
            // TODO: check for deterministic predicted, pre-predicted, pre-spawned
            let predicted = local_entity_mut.get::<Predicted>().is_some();
            let interpolated = local_entity_mut.get::<Interpolated>().is_some();

            // the local Sender has authority over the entity, so we don't want to accept the updates
            if local_entity_mut
                .get::<ReplicationState>()
                .as_ref()
                .is_some_and(|s| s.has_authority(receiver_entity))
            {
                trace!(
                    "Ignored a replication action received from peer {:?} since the receiver has authority over the entity: {:?}",
                    remote, entity
                );
                continue;
            }

            let mut buffered_entity = BufferedEntity {
                entity: local_entity_mut,
                buffered: temp_write_buffer,
            };

            // inserts
            // TODO: remove updates that are duplicate for the same component
            let _ = actions
                .insert
                .into_iter()
                .try_for_each(|bytes| {
                    component_registry.buffer(
                        bytes,
                        &mut buffered_entity,
                        remote_tick,
                        &mut remote_entity_map.remote_to_local,
                        predicted,
                        interpolated,
                    )
                })
                .inspect_err(|e| error!("could not insert the components to the entity: {:?}", e));

            // removals
            actions.remove.into_iter().for_each(|component_net_id| {
                component_registry.remove(
                    component_net_id,
                    &mut buffered_entity,
                    predicted,
                    interpolated,
                    remote_tick,
                );
            });

            buffered_entity.apply();

            // updates
            for component in actions.updates {
                let _ = component_registry
                    .buffer(
                        component,
                        &mut buffered_entity,
                        remote_tick,
                        &mut remote_entity_map.remote_to_local,
                        predicted,
                        interpolated,
                    )
                    .inspect_err(|e| {
                        error!("could not write the component to the entity: {:?}", e)
                    });
            }

            buffered_entity.apply();
        }

        // Flush commands because the entities that were inserted might have triggered some observers
        // In particular, the PreSpawned component triggers an observer that inserts Confirmed, and
        // we want Confirmed to be added so that it can be updated with the correct tick!
        world.flush();

        // TODO: apply authority check for the update confirmed tick?
        self.update_confirmed_tick(world, group_id, remote_tick);
    }

    pub(crate) fn apply_updates_message(
        &mut self,
        world: &mut World,
        receiver_entity: Entity,
        remote: PeerId,
        component_registry: &ComponentRegistry,
        remote_tick: Tick,
        is_history: bool,
        message: UpdatesMessage,
        remote_entity_map: &mut RemoteEntityMap,
    ) {
        let group_id = message.group_id;
        let is_default_group = group_id == ReplicationGroupId(0);
        // TODO: store this in ConfirmedHistory?
        if is_history {
            return;
        }
        trace!(
            ?remote_tick,
            ?message,
            "Received replication updates from remote: {:?}",
            remote
        );
        for (entity, components) in message.into_iter() {
            trace!(?components, remote_entity = ?entity, "Received UpdateComponent");
            let Some(mut local_entity_mut) = remote_entity_map.get_by_remote(world, entity) else {
                // we can get a few buffered updates after the entity has been despawned
                // those are the updates that we received before the despawn action message, but with a tick
                // later than the despawn action message
                info!(remote_entity = ?entity, "update for entity that doesn't exist?");
                continue;
            };
            let local_entity = local_entity_mut.id();
            // for the default group: update the ConfirmedTick now
            if is_default_group
                && let Some(mut confirmed) = local_entity_mut.get_mut::<ConfirmedTick>()
            {
                trace!(
                    ?remote_tick,
                    ?local_entity,
                    "updating confirmed tick for entity"
                );
                confirmed.tick = remote_tick;
            }

            // TODO: check for deterministic predicted, pre-predicted, pre-spawned ?
            let predicted = local_entity_mut.get::<Predicted>().is_some();
            let interpolated = local_entity_mut.get::<Interpolated>().is_some();

            // the local Sender has authority over the entity, so we don't want to accept the updates
            if local_entity_mut
                .get::<ReplicationState>()
                .as_ref()
                .is_some_and(|s| s.has_authority(receiver_entity))
            {
                trace!(
                    "Ignored a replication action received from peer {:?} since the receiver has authority over the entity: {:?}",
                    remote, entity
                );
                continue;
            }

            let mut local_entity_mut = BufferedEntity {
                entity: local_entity_mut,
                buffered: &mut BufferedChanges::default(),
            };
            for component in components {
                let _ = component_registry
                    .buffer(
                        component,
                        &mut local_entity_mut,
                        remote_tick,
                        &mut remote_entity_map.remote_to_local,
                        predicted,
                        interpolated,
                    )
                    .inspect_err(|e| {
                        error!("could not write the component to the entity: {:?}", e)
                    });
            }

            local_entity_mut.apply();
        }

        // Flush commands because the entities that were inserted might have triggered some observers
        // In particular, the PreSpawned component triggers an observer that inserts Confirmed, and
        // we want Confirmed to be added so that it can be updated with the correct tick!
        world.flush();

        // TODO: should the update_confirmed_tick only be for entities in the group for which we have authority?
        self.update_confirmed_tick(world, group_id, remote_tick);
    }

    /// Update the Confirmed tick for all entities in the replication group
    /// so that Predicted/Interpolated entities can be notified
    ///
    /// We update it for all entities in the group (even if we received only an update that contains
    /// updates for E1, it also means that E2 is updated to the same tick, since they are part of the
    /// same group)
    pub(crate) fn update_confirmed_tick(
        &mut self,
        world: &mut World,
        group_id: ReplicationGroupId,
        remote_tick: Tick,
    ) {
        trace!(
            ?remote_tick,
            "Updating confirmed tick for entities {:?} in group: {:?}",
            self.local_entities,
            group_id
        );
        self.local_entities.iter().for_each(|local_entity| {
            if let Ok(mut local_entity_mut) = world.get_entity_mut(*local_entity)
                && let Some(mut confirmed) = local_entity_mut.get_mut::<ConfirmedTick>()
            {
                trace!(
                    ?remote_tick,
                    ?local_entity,
                    "updating confirmed tick for entity"
                );
                confirmed.tick = remote_tick;
            }
        });
    }
}
