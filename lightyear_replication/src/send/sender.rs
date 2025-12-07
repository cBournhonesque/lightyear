use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::messages::actions::Actions;
use crate::messages::updates::Updates;
use crate::registry::registry::ComponentRegistry;
use crate::registry::{ComponentKind, ComponentNetId};
use crate::send::sender_ticks::SenderTicks;
use alloc::{string::ToString, vec::Vec};
use bevy_ecs::{
    component::{Component, Tick as BevyTick},
    entity::{Entity, EntityHash},
};
use bevy_platform::collections::{HashMap, HashSet};
use bevy_ptr::Ptr;
use bevy_reflect::Reflect;
use bevy_time::{Timer, TimerMode};
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::{RemoteEntityMap, SendEntityMap};
use lightyear_serde::writer::Writer;
use lightyear_transport::packet::message::MessageId;
use lightyear_transport::prelude::Transport;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

type EntityHashMap<K, V> = HashMap<K, V, EntityHash>;
type EntityHashSet<K> = HashSet<K, EntityHash>;

#[derive(Clone, Copy, Debug, Reflect)]
pub enum SendUpdatesMode {
    /// We send all the updates that happened since the last tick when we received an ACK from the remote
    ///
    /// E.g. if the component was updated at tick 3; we will send the update at tick 3, and then at tick 4,
    /// we will send the update again even if the component wasn't updated, because we still haven't
    /// received an ACK from the client.
    SinceLastAck,
    // TODO: this is currently bugged because we need to maintain a `send_tick` / `ack_tick` per (entity, component)
    /// We send all the updates that happened since the last tick where we **sent** an update.
    /// E.g. if the component was updated at tick 3; we will send the update at tick 3, and then at tick 4,
    /// we won't be sending anything since the component wasn't updated after that.
    ///
    /// 99% of the time the packets don't get lost so this is fine to do, and allows us to save bandwidth
    /// by not sending the same update multiple time.
    ///
    /// If we receive a NACK (i.e. the packet got lost), we will send the updates since the last ACK.
    SinceLastSend,
}

#[derive(Component)]
#[require(Transport)]
#[require(LocalTimeline)]
pub struct ReplicationSender {
    // track entities that were recently spawned on this sender, so that we can update ReplicationState after `replicate`
    // this would not be needed if we used DashMap within ReplicationState
    pub(crate) new_spawns: Vec<Entity>,
    pub(crate) pending_despawns: Vec<Entity>,
    pub(crate) pending_actions: Actions,
    pub(crate) pending_updates: Updates,
    pub(crate) sender_ticks: SenderTicks,
    pub(crate) writer: Writer,
    pub send_timer: Timer,
    pub(crate) send_updates_mode: SendUpdatesMode,
    // TODO: detect automatically if priority manager is enabled!
    pub(crate) bandwidth_cap_enabled: bool,
}

impl Default for ReplicationSender {
    fn default() -> Self {
        Self::new(Duration::default(), SendUpdatesMode::SinceLastAck, false)
    }
}

impl ReplicationSender {
    pub fn new(
        send_interval: Duration,
        send_updates_mode: SendUpdatesMode,
        bandwidth_cap_enabled: bool,
    ) -> Self {
        // make sure that the timer is finished when we start, to immediately start replicating
        let mut send_timer = Timer::new(send_interval, TimerMode::Repeating);
        send_timer.tick(Duration::MAX);
        Self {
            // SEND
            new_spawns: Vec::default(),
            pending_despawns: Vec::default(),
            writer: Writer::default(),
            send_updates_mode,
            // PRIORITY
            send_timer,
            bandwidth_cap_enabled,
        }
    }

    // /// Returns true if the `Tick` was updated since the last time the Sender was buffering replication updates
    // #[inline(always)]
    // pub(crate) fn is_updated(&self, tick: BevyTick) -> bool {
    //     self.this_run == self.last_run || tick.is_newer_than(self.last_run, self.this_run)
    // }

    pub fn send_interval(&self) -> Duration {
        self.send_timer.duration()
    }

    /// Mark an entity as needing to be despawned if it was previously replicated-spawned by this sender
    pub(crate) fn set_replicated_despawn(&mut self, entity: Entity) {
        self.pending_despawns.push(entity);
    }

    /// Internal bookkeeping:
    /// 1. handle all nack update messages (by resetting the send_tick to the previous ack_tick)
    pub(crate) fn handle_nacks(&mut self, world_tick: BevyTick, update_nacks: &mut Vec<MessageId>) {
        // 1. handle all nack update messages
        update_nacks.drain(..).for_each(|message_id| {
            // remember to remove the entry from the map to avoid memory leakage
            match self.updates_message_id_to_group_id.remove(&message_id)
            { Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                ..
            }) => {
                if let SendUpdatesMode::SinceLastSend = self.send_updates_mode {
                    match self.group_channels.get_mut(&group_id) { Some(channel) => {
                        // when we know an update message has been lost, we need to reset our send_tick
                        // to our previous ack_tick
                        trace!(
                            "Update channel send_tick back to ack_tick because a message has been lost"
                        );
                        // only reset the send tick if the bevy_tick of the message that was lost is
                        // newer than the current ack_tick
                        // (otherwise it just means we lost some old message, and we don't need to do anything)
                        if channel
                            .ack_bevy_tick
                            .is_some_and(|ack_tick| bevy_tick.is_newer_than(ack_tick, world_tick))
                        {
                            channel.send_tick = channel.ack_bevy_tick;
                        }

                        // TODO: if all clients lost a given message, than we can immediately drop the
                        //  delta-compression data for that tick
                    } _ => {
                        error!("Received an update message-id nack but the corresponding group channel does not exist");
                    }}
                }
            } _ => {
                // NOTE: this happens when a message-id is split between multiple packets (fragmented messages)
                trace!("Received an update message-id nack ({message_id:?}) but we don't know the corresponding group id");
            }}
        })
    }

    /// If we got notified that an update got send (included in a packet):
    /// - we reset the accumulated priority to 0.0 for all replication groups included in the message
    /// - we update the replication groups' send_tick
    ///   Then we accumulate the priority for all replication groups.
    ///
    /// This should be call after the Send SystemSet.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn recv_send_notification(&mut self, messages_sent: &mut Vec<MessageId>) {
        if !self.bandwidth_cap_enabled {
            return;
        }
        messages_sent.drain(..).for_each(|message_id| {
            match self.updates_message_id_to_group_id.get(&message_id)
            { Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                ..
            }) => {
                match self.group_channels.get_mut(group_id) { Some(channel) => {
                    // TODO: should we also reset the priority for replication-action messages?
                    // reset the priority
                    debug!(
                        ?message_id,
                        ?group_id,
                        "successfully sent message for replication group! Updating send_tick"
                    );
                    channel.send_tick = Some(*bevy_tick);
                    channel.accumulated_priority = 0.0;
                } _ => {
                    error!(?message_id, ?group_id, "Received a send message-id notification but the corresponding group channel does not exist");
                }}
            } _ => {
                error!(?message_id,
                    "Received an send message-id notification but we don't know the corresponding group id"
                );
            }}
        })
    }

    /// Handle a notification that a message got acked:
    /// - update the channel's ack_tick and ack_bevy_tick
    ///
    /// We call this after the Receive SystemSet; to update the bevy_tick at which we received entity updates for each group
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn handle_acks(
        &mut self,
        component_registry: &ComponentRegistry,
        delta_manager: Option<&DeltaManager>,
        update_acks: &mut Vec<MessageId>,
    ) {
        update_acks.drain(..).for_each(|message_id| {
            // TODO: lost messages would result in memory-leakage here, since
            //  we would never remove them from this map!!!

            // remember to remove the entry from the map to avoid memory leakage
            match self.updates_message_id_to_group_id.remove(&message_id)
            { Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                tick,
                delta,
            }) => {
                match self.group_channels.get_mut(&group_id) { Some(channel) => {
                    // update the ack tick for the channel
                    trace!(?group_id, ?bevy_tick, ?tick, ?delta, "Update channel ack_tick");
                    channel.ack_bevy_tick = Some(bevy_tick);
                    // `delta_ack_ticks` won't grow indefinitely thanks to the cleanup systems
                    for (entity, component_kind) in delta {
                        channel
                            .delta_ack_ticks
                            .insert((entity, component_kind), tick);
                        delta_manager.as_ref().unwrap().receive_ack(entity, tick, component_kind, component_registry);
                    }
                } _ => {
                    error!("Received an update message-id ack but the corresponding group channel does not exist");
                }}
            } _ => {
                error!("Received an update message-id ack but we don't know the corresponding group id");
            }}
        })
    }

    /// Do some internal bookkeeping:
    /// - handle tick wrapping
    pub(crate) fn tick_cleanup(&mut self, tick: Tick) {
        // skip cleanup if we did one recently
        if self
            .last_cleanup_tick
            .is_some_and(|last| tick < last + (i16::MAX / 3))
        {
            return;
        }
        self.last_cleanup_tick = Some(tick);
        let delta = i16::MAX / 2;
        // if it's been enough time since we last any action for the group, we can set the last_action_tick to None
        // (meaning that there's no need when we receive the update to check if we have already received a previous action)
        for group_channel in self.group_channels.values_mut() {
            if let Some(last_action_tick) = group_channel.last_action_tick
                && tick - last_action_tick > delta
            {
                debug!(
                    ?tick,
                    ?last_action_tick,
                    ?group_channel,
                    "Setting the last_action tick to None because there hasn't been any new actions in a while"
                );
                group_channel.last_action_tick = None;
            }
            group_channel
                .delta_ack_ticks
                .retain(|_, ack_tick| tick - *ack_tick <= delta);
        }
    }
}

impl ReplicationSender {
    /// Create a component update for a component that has delta compression enabled
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_delta_component_update(
        &mut self,
        entity: Entity,
        mapped_entity: Entity,
        group_id: ReplicationGroupId,
        kind: ComponentKind,
        component_data: Ptr,
        registry: &ComponentRegistry,
        delta_manager: &DeltaManager,
        _tick: Tick,
        remote_entity_map: &mut RemoteEntityMap,
    ) -> Result<(), ReplicationError> {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::send::component_update_delta").increment(1);
        }
        let group_channel = self.group_channels.entry(group_id).or_default();
        // Get the latest acked tick for this entity/component
        let raw_data = group_channel
            .delta_ack_ticks
            .get(&(entity, kind))
            .map(|&ack_tick| {
                // we have an ack tick for this replication group, get the corresponding component value
                // so we can compute a diff
                let old_data = delta_manager
                    // NOTE: remember to use the local entity for local bookkeeping
                    .get(entity, ack_tick, kind)
                    .ok_or(ReplicationError::DeltaCompressionError(
                        "could not find old component value to compute delta".to_string(),
                    ))
                    .inspect_err(|e| {
                        error!(
                            ?entity,
                            "Could not find old component value from tick {:?} to compute delta: {e:?}",
                            ack_tick,
                        );
                        error!("DeltaManager: {:?}", delta_manager);
                    })?;
                // SAFETY: the component_data and erased_data is a pointer to a component that corresponds to kind
                unsafe {
                    registry.serialize_diff(
                        ack_tick,
                        old_data,
                        component_data,
                        &mut self.writer,
                        kind,
                        &mut remote_entity_map.local_to_remote,
                    )?;
                }
                Ok::<Bytes, ReplicationError>(self.writer.split())
            })
            .unwrap_or_else(|| {
                // SAFETY: the component_data is a pointer to a component that corresponds to kind
                unsafe {
                    // compute a diff from the base value, and serialize that
                    registry.serialize_diff_from_base_value(
                        component_data,
                        &mut self.writer,
                        kind,
                        &mut remote_entity_map.local_to_remote,
                    )?;
                }
                Ok::<Bytes, ReplicationError>(self.writer.split())
            })?;
        trace!(?kind, "Inserting pending update!");
        // use the network entity when serializing
        group_channel
            .pending_delta_updates
            .push((mapped_entity, kind));
        self.prepare_component_update(mapped_entity, group_id, raw_data);
        Ok(())
    }
}

/// Channel to keep track of sending replication messages for a given Group
#[derive(Debug)]
pub struct GroupChannel {
    /// List of (Entity, Component) pairs for which we write a delta update
    pub pending_delta_updates: Vec<(Entity, ComponentKind)>,

    pub actions_next_send_message_id: MessageId,

    // TODO: maybe also keep track of which Tick this bevy-tick corresponds to? (will enable doing diff-compression)
    /// Bevy Tick when we last sent an update for this group.
    /// This is used to collect updates that we will replicate; we replicate any update that happened after this tick.
    /// (and not after the last ack_tick, because 99% of the time the packet won't be lost so there is no need
    /// to wait for an ack. If we keep sending updates since the last ack, we would be sending a lot of duplicate messages)
    ///
    /// at the start, it's `None` (meaning that we send any changes)
    pub send_tick: Option<BevyTick>,
    /// Bevy Tick when we last received an ack for an update message for this group.
    ///
    /// If a message is acked, we bump the ack_tick to the `send_tick` at which we sent the update.
    /// (meaning that we don't need to send updates that happened before that `send_tick` anymore)
    ///
    /// If a message is lost, we bump the `send_tick` back to the `ack_tick`, because we might need to re-send those updates.
    pub ack_bevy_tick: Option<BevyTick>,
    /// For delta compression, we need to keep the last ack-tick that we compute the diff from
    /// for each (entity, component) pair.
    /// Keeping a tick for the entire replication group is not enough.
    /// For example:
    /// - tick 1: send C1A
    /// - tick 2: send C2. After it's received, ack_tick = 2
    /// - tick 3: send C1B as diff-C1A-C1B. The receiver cannot process it if the ack_tick = 2, because the receiver stored (C1A, tick 1) in its buffer
    ///
    /// Another solution might be that the receiver also only keeps track of a single ack tick
    /// for the entire replication group, but that needs to be fleshed out more.
    pub delta_ack_ticks: HashMap<(Entity, ComponentKind), Tick>,

    /// Last tick for which we sent an action message. Needed because we want the receiver to only
    /// process Updates if they have processed all Actions that happened before them.
    pub last_action_tick: Option<Tick>,

    /// The priority to send the replication group.
    /// This will be reset to base_priority every time we send network updates, unless we couldn't send a message
    /// for this group because of the bandwidth cap, in which case it will be accumulated.
    pub accumulated_priority: f32,
    pub base_priority: f32,
}

impl Default for GroupChannel {
    fn default() -> Self {
        Self {
            pending_delta_updates: Vec::default(),
            actions_next_send_message_id: MessageId(0),
            send_tick: None,
            ack_bevy_tick: None,
            delta_ack_ticks: HashMap::default(),
            last_action_tick: None,
            accumulated_priority: 0.0,
            base_priority: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use lightyear_transport::prelude::{ChannelMode, ChannelRegistry, ChannelSettings};

    #[cfg(feature = "std")]
    use test_log::test;

    fn setup() -> (ReplicationSender, Transport) {
        let mut channel_registry = ChannelRegistry::default();
        channel_registry.add_channel::<UpdatesChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliableWithAcks,
            // we do not send the send_frequency to `replication_interval` here
            // because we want to make sure that the entity updates for tick T
            // are sent on tick T, so we will set the `replication_interval`
            // directly on the replication_sender
            send_frequency: Duration::default(),
            priority: 1.0,
        });
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<UpdatesChannel>(&channel_registry);
        let sender =
            ReplicationSender::new(Duration::default(), SendUpdatesMode::SinceLastSend, false);
        (sender, transport)
    }

    /// Test that in mode SinceLastSend, the `send_tick` is updated correctly:
    /// - updated immediately after sending a message
    /// - reverts back to the `ack_tick` when a message is lost
    #[test]
    fn test_send_tick_no_priority() {
        let (mut sender, mut transport) = setup();

        let entity = Entity::from_bits(1);
        let group_1 = ReplicationGroupId(0);
        sender
            .group_channels
            .insert(group_1, GroupChannel::default());

        let message_1 = MessageId(0);
        let message_2 = MessageId(1);
        let message_3 = MessageId(2);
        let bevy_tick_1 = BevyTick::new(0);
        let bevy_tick_2 = BevyTick::new(2);
        let bevy_tick_3 = BevyTick::new(4);
        let tick_1 = Tick(0);
        let tick_2 = Tick(2);
        let tick_3 = Tick(4);

        // when we buffer a message to be sent, we update the `send_tick`
        sender.prepare_component_update(entity, group_1, Bytes::new());
        sender
            .send_updates_messages(tick_1, bevy_tick_1, &mut transport, MessageNetId::default())
            .unwrap();

        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_1),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_1,
                tick: tick_1,
                delta: vec![],
            })
        );
        assert_eq!(group.send_tick, Some(bevy_tick_1));
        assert_eq!(group.ack_bevy_tick, None);

        // if we buffer a second message, we update the `send_tick`
        sender.prepare_component_update(entity, group_1, Bytes::new());
        sender
            .send_updates_messages(tick_2, bevy_tick_2, &mut transport, MessageNetId::default())
            .unwrap();
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_2),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_2,
                tick: tick_2,
                delta: vec![],
            })
        );
        assert_eq!(group.send_tick, Some(bevy_tick_2));
        assert_eq!(group.ack_bevy_tick, None);

        // if we receive an ack for the second message, we update the `ack_tick`
        let mut delta_manager = DeltaManager::default();
        let component_registry = ComponentRegistry::default();
        sender.handle_acks(
            &component_registry,
            Some(&mut delta_manager),
            &mut vec![message_2],
        );
        let group = sender.group_channels.get(&group_1).unwrap();
        assert!(
            !sender
                .updates_message_id_to_group_id
                .contains_key(&message_2)
        );
        assert_eq!(group.send_tick, Some(bevy_tick_2));
        assert_eq!(group.ack_bevy_tick, Some(bevy_tick_2));

        // if we buffer a third message, we update the `send_tick`
        sender.prepare_component_update(entity, group_1, Bytes::new());
        sender
            .send_updates_messages(tick_3, bevy_tick_3, &mut transport, MessageNetId::default())
            .unwrap();
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_3),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_3,
                tick: tick_3,
                delta: vec![],
            })
        );
        assert_eq!(group.send_tick, Some(bevy_tick_3));
        assert_eq!(group.ack_bevy_tick, Some(bevy_tick_2));

        // if we receive a nack for the first message, we don't care because that message's bevy tick
        // is lower than our ack tick
        sender.handle_nacks(BevyTick::new(10), &mut vec![message_1]);
        // make sure that the send tick wasn't updated
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(group.send_tick, Some(bevy_tick_3));

        // however if we receive a nack for the third message, we update the `send_tick` back to the `ack_tick`
        sender.handle_nacks(BevyTick::new(10), &mut vec![message_3]);
        let group = sender.group_channels.get(&group_1).unwrap();
        assert!(
            !sender
                .updates_message_id_to_group_id
                .contains_key(&message_3),
        );
        // this time the `send_tick` is updated to the `ack_tick`
        assert_eq!(group.send_tick, Some(bevy_tick_2));
        assert_eq!(group.ack_bevy_tick, Some(bevy_tick_2));
    }

    #[test]
    fn test_send_tick_priority() {
        let (mut sender, mut transport) = setup();
        sender.bandwidth_cap_enabled = true;

        let entity = Entity::from_bits(1);
        let group_1 = ReplicationGroupId(0);
        sender
            .group_channels
            .insert(group_1, GroupChannel::default());

        let message_1 = MessageId(0);
        let bevy_tick_1 = BevyTick::new(0);
        let tick_1 = Tick(0);

        // when we buffer a message to be sent, we don't update the `send_tick`
        // (because we wait until the message is actually send)
        sender.prepare_component_update(entity, group_1, Bytes::new());
        sender
            .send_updates_messages(tick_1, bevy_tick_1, &mut transport, MessageNetId::default())
            .unwrap();
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_1),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_1,
                tick: tick_1,
                delta: vec![],
            })
        );
        assert_eq!(group.send_tick, None);
        assert_eq!(group.ack_bevy_tick, None);

        // when the message is actually sent, we update the `send_tick`
        sender.recv_send_notification(&mut vec![message_1]);
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(group.send_tick, Some(bevy_tick_1));
        assert_eq!(group.ack_bevy_tick, None);
    }
}
