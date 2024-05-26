//! General struct handling replication
use std::any::Any;
use std::collections::BTreeMap;
use std::iter::Extend;
use std::ptr::NonNull;

use crate::channel::builder::{EntityActionsChannel, EntityUpdatesChannel};
use anyhow::Context;
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::{Component, Entity, Reflect};
use bevy::ptr::{OwningPtr, Ptr, PtrMut};
use bevy::utils::petgraph::data::ElementIterator;
use bevy::utils::{hashbrown, HashMap, HashSet};
use crossbeam_channel::Receiver;
use hashbrown::hash_map::Entry;
use tracing::{debug, error, info, trace, warn};

use crate::packet::message::MessageId;
use crate::prelude::{ComponentRegistry, ShouldBePredicted, Tick};
use crate::protocol::channel::ChannelKind;
use crate::protocol::component::{ComponentKind, ComponentNetId};
use crate::protocol::registry::NetId;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::RawData;
use crate::shared::replication::components::ReplicationGroupId;
use crate::shared::replication::delta::Diffable;
use crate::utils::ready_buffer::ReadyBuffer;

use super::{
    EntityActionMessage, EntityActions, EntityUpdatesMessage, ReplicationMessageData, SpawnAction,
};

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

type EntityHashSet<K> = hashbrown::HashSet<K, EntityHash>;

/// When a [`EntityUpdatesMessage`] message gets buffered (and we have access to its [`MessageId`]),
/// we keep track of some information related to this message.
/// It is useful when we get notified that the message was acked or lost.
#[derive(Debug, PartialEq)]
struct UpdateMessageMetadata {
    /// The group id that this message is about
    group_id: ReplicationGroupId,
    /// The BevyTick at which we buffered the message
    bevy_tick: BevyTick,
    /// The tick at which we buffered the message
    tick: Tick,
}

#[derive(Debug)]
pub(crate) struct ReplicationSender {
    /// Get notified whenever a message-id that was sent has been received by the remote
    pub(crate) updates_ack_receiver: Receiver<MessageId>,
    /// Get notified whenever a message-id that was sent has been lost by the remote
    pub(crate) updates_nack_receiver: Receiver<MessageId>,

    /// Map from message-id to the corresponding group-id that sent this update message, as well as the `send_tick` BevyTick
    /// when we buffered the message. (so that when it's acked, we know we only need to include updates that happened after that tick,
    /// for that replication group)
    pub(crate) updates_message_id_to_group_id: HashMap<MessageId, UpdateMessageMetadata>,
    /// Messages that are being written. We need to hold a buffer of messages because components actions/updates
    /// are being buffered individually but we want to group them inside a message
    pub pending_actions: EntityHashMap<ReplicationGroupId, EntityHashMap<Entity, EntityActions>>,
    pub pending_updates: EntityHashMap<ReplicationGroupId, EntityHashMap<Entity, Vec<RawData>>>,
    // Set of unique components for each entity, to avoid sending multiple updates/inserts for the same component
    // pub pending_unique_components:
    //     EntityHashMap<ReplicationGroupId, EntityHashMap<Entity, HashSet<ComponentNetId>>>,
    /// Buffer to so that we have an ordered receiver per group
    pub group_channels: EntityHashMap<ReplicationGroupId, GroupChannel>,

    // PRIORITY
    /// Get notified whenever a message for a given ReplicationGroup was actually sent
    /// (sometimes they might not be sent because of bandwidth constraints)
    ///
    /// We update the `send_tick` only when the message was actually sent.
    pub message_send_receiver: Receiver<MessageId>,

    /// By default, we will send all component updates since the last time we sent an update for a given entity.
    /// E.g. if the component was updated at tick 3; we will send the update at tick 3, and then at tick 4,
    /// we won't be sending anything since the component wasn't updated after that.
    ///
    /// This helps save bandwidth, but can cause the client to have delayed eventual consistency in the
    /// case of packet loss.
    ///
    /// If this is set to true, we will instead send all updates since the last time we received an ACK from the client.
    /// E.g. if the component was updated at tick 3; we will send the update at tick 3, and then at tick 4,
    /// we will send the update again even if the component wasn't updated, because we still haven't
    /// received an ACK from the client.
    send_updates_since_last_ack: bool,
    bandwidth_cap_enabled: bool,
}

impl ReplicationSender {
    pub(crate) fn new(
        updates_ack_receiver: Receiver<MessageId>,
        updates_nack_receiver: Receiver<MessageId>,
        message_send_receiver: Receiver<MessageId>,
        send_updates_since_last_ack: bool,
        bandwidth_cap_enabled: bool,
    ) -> Self {
        Self {
            // SEND
            updates_ack_receiver,
            updates_nack_receiver,
            updates_message_id_to_group_id: Default::default(),
            pending_actions: EntityHashMap::default(),
            pending_updates: EntityHashMap::default(),
            // pending_unique_components: EntityHashMap::default(),
            group_channels: Default::default(),
            send_updates_since_last_ack,
            // PRIORITY
            message_send_receiver,
            bandwidth_cap_enabled,
        }
    }

    /// Keep track of the message_id/bevy_tick/tick where a replication-update message has been sent
    /// for a given group
    pub(crate) fn buffer_replication_update_message(
        &mut self,
        group_id: ReplicationGroupId,
        message_id: MessageId,
        bevy_tick: BevyTick,
        tick: Tick,
    ) {
        self.updates_message_id_to_group_id.insert(
            message_id,
            UpdateMessageMetadata {
                group_id,
                bevy_tick,
                tick,
            },
        );
        // If we don't have a bandwidth cap, buffering a message is equivalent to sending it
        // so we can set the `send_tick` right away
        if !self.bandwidth_cap_enabled {
            if let Some(channel) = self.group_channels.get_mut(&group_id) {
                channel.send_tick = Some(bevy_tick);
            }
        }
    }

    /// Get the `send_tick` for a given group.
    /// We will send all updates that happened after this bevy tick.
    pub(crate) fn get_send_tick(&self, group_id: ReplicationGroupId) -> Option<BevyTick> {
        self.group_channels.get(&group_id).and_then(|channel| {
            if self.send_updates_since_last_ack {
                channel.ack_tick
            } else {
                channel.send_tick
            }
        })
    }

    /// Internal bookkeeping:
    /// 1. handle all nack update messages
    pub(crate) fn update(&mut self, world_tick: BevyTick) {
        // 1. handle all nack update messages
        while let Ok(message_id) = self.updates_nack_receiver.try_recv() {
            // remember to remove the entry from the map to avoid memory leakage
            if let Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                tick,
            }) = self.updates_message_id_to_group_id.remove(&message_id)
            {
                if let Some(channel) = self.group_channels.get_mut(&group_id) {
                    // when we know an update message has been lost, we need to reset our send_tick
                    // to our previous ack_tick
                    trace!(
                        "Update channel send_tick back to ack_tick because a message has been lost"
                    );
                    // only reset the send tick if the bevy_tick of the message that was lost is
                    // newer than the current ack_tick
                    // (otherwise it just means we lost some old message, and we don't need to do anything)
                    if channel
                        .ack_tick
                        .is_some_and(|ack_tick| bevy_tick.is_newer_than(ack_tick, world_tick))
                    {
                        channel.send_tick = channel.ack_tick;
                    }

                    // we can drop the diff data for that tick since we will never be able to compute diffs from it
                    channel
                        .delta_component_values
                        .values_mut()
                        .for_each(|entry| {
                            entry.component_data.remove(&tick);
                        });
                } else {
                    error!("Received an update message-id nack but the corresponding group channel does not exist");
                }
            } else {
                error!("Received an update message-id nack but we don't know the corresponding group id");
            }
        }
    }

    /// If we got notified that an update got send (included in a packet):
    /// - we reset the accumulated priority to 0.0 for all replication groups included in the message
    /// - we update the replication groups' send_tick
    /// Then we accumulate the priority for all replication groups.
    ///
    /// This should be call after the Send SystemSet.
    pub(crate) fn recv_send_notification(&mut self) {
        if !self.bandwidth_cap_enabled {
            return;
        }
        // TODO: handle errors that are not channel::isEmpty
        while let Ok(message_id) = self.message_send_receiver.try_recv() {
            if let Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                ..
            }) = self.updates_message_id_to_group_id.get(&message_id)
            {
                if let Some(channel) = self.group_channels.get_mut(group_id) {
                    // TODO: should we also reset the priority for replication-action messages?
                    // reset the priority
                    debug!(
                        ?message_id,
                        ?group_id,
                        "successfully sent message for replication group! Resetting priority"
                    );
                    channel.send_tick = Some(*bevy_tick);
                    channel.accumulated_priority = Some(0.0);
                } else {
                    error!(?message_id, ?group_id, "Received a send message-id notification but the corresponding group channel does not exist");
                }
            } else {
                error!(?message_id,
                    "Received an send message-id notification but we know the corresponding group id"
                );
            }
        }

        // then accumulate the priority for all replication groups
        self.group_channels.values_mut().for_each(|channel| {
            channel.accumulated_priority = channel
                .accumulated_priority
                .map_or(Some(channel.base_priority), |acc| {
                    Some(acc + channel.base_priority)
                });
        });
    }

    // TODO: call this in a system after receive?
    /// We call this after the Receive SystemSet; to update the bevy_tick at which we received entity updates for each group
    pub(crate) fn recv_update_acks(&mut self, component_registry: &ComponentRegistry) {
        // TODO: handle errors that are not channel::isEmpty
        while let Ok(message_id) = self.updates_ack_receiver.try_recv() {
            // remember to remove the entry from the map to avoid memory leakage
            if let Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                tick,
            }) = self.updates_message_id_to_group_id.remove(&message_id)
            {
                if let Some(channel) = self.group_channels.get_mut(&group_id) {
                    // update the ack tick for the channel
                    debug!(?bevy_tick, "Update channel ack_tick");
                    channel.ack_tick = Some(bevy_tick);

                    // TODO: maybe optimize this by keeping track in each message of which delta compression components were included?
                    // update delta-compression data
                    channel
                        .delta_component_values
                        .iter_mut()
                        .for_each(|(kind, entry)| {
                            // if we sent a message at that tick, we can set ack to true (it
                            // will stay true from now on)
                            if entry.component_data.contains_key(&tick) {
                                entry.acked = true;
                                // we can remove all the keys older than the acked key
                                let new_data =
                                    entry.component_data.split_off(&tick).into_iter().collect();
                                // call drop on all the data that we are removing
                                entry.component_data.values_mut().for_each(|data| {
                                    data.values_mut().for_each(|owned_ptr| unsafe {
                                        // SAFETY: we have unique ownership of the data
                                        let ptr = PtrMut::new(*owned_ptr);
                                        // SAFETY: the ptr corresponds the component associated with kind
                                        component_registry.erased_drop(ptr, *kind).unwrap();
                                    });
                                });
                                // only keep the data that is more recent (inclusive) than the acked tick
                                entry.component_data = new_data;
                            }
                        });
                } else {
                    error!("Received an update message-id ack but the corresponding group channel does not exist");
                }
            } else {
                error!("Received an update message-id ack but we don't know the corresponding group id");
            }
        }
    }

    /// Do some internal bookkeeping:
    /// - handle tick wrapping
    pub(crate) fn cleanup(&mut self, tick: Tick) {
        // if it's been enough time since we last any action for the group, we can set the last_action_tick to None
        // (meaning that there's no need when we receive the update to check if we have already received a previous action)
        for group_channel in self.group_channels.values_mut() {
            debug!("Checking group channel: {:?}", group_channel);
            if let Some(last_action_tick) = group_channel.last_action_tick {
                if tick - last_action_tick > (i16::MAX / 2) {
                    debug!(
                    ?tick,
                    ?last_action_tick,
                    ?group_channel,
                    "Setting the last_action tick to None because there hasn't been any new actions in a while");
                    group_channel.last_action_tick = None;
                }
            }
        }
    }
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
///
/// - all component inserts/removes/updates for an entity to be grouped together in a single message
impl ReplicationSender {
    /// Update the base priority for a given group
    pub(crate) fn update_base_priority(&mut self, group_id: ReplicationGroupId, priority: f32) {
        let channel = self.group_channels.entry(group_id).or_default();
        channel.base_priority = priority;
        // if we already have an accumulated priority, don't override it
        if channel.accumulated_priority.is_none() {
            channel.accumulated_priority = Some(priority);
        }
    }

    // TODO: how can I emit metrics here that contain the channel kind?
    //  use a OnceCell that gets set with the channel name mapping when the protocol is finalized?
    //  the other option is to have wrappers in Connection, but that's pretty ugly

    /// Host has spawned an entity, and we want to replicate this to remote
    /// Returns true if we should send a message
    pub(crate) fn prepare_entity_spawn(&mut self, entity: Entity, group_id: ReplicationGroupId) {
        self.pending_actions
            .entry(group_id)
            .or_default()
            .entry(entity)
            .or_default()
            .spawn = SpawnAction::Spawn;
    }

    /// Host wants to start replicating an entity, but instead of spawning a new entity, it wants to reuse an existing entity
    /// on the remote. This can be useful for transferring ownership of an entity from one player to another.
    pub(crate) fn prepare_entity_spawn_reuse(
        &mut self,
        local_entity: Entity,
        group_id: ReplicationGroupId,
        remote_entity: Entity,
    ) {
        self.pending_actions
            .entry(group_id)
            .or_default()
            .entry(local_entity)
            .or_default()
            .spawn = SpawnAction::Reuse(remote_entity.to_bits());
    }

    pub(crate) fn prepare_entity_despawn(&mut self, entity: Entity, group_id: ReplicationGroupId) {
        self.pending_actions
            .entry(group_id)
            .or_default()
            .entry(entity)
            .or_default()
            .spawn = SpawnAction::Despawn;
    }

    // we want to send all component inserts that happen together for the same entity in a single message
    // (because otherwise the inserts might be received at different packets/ticks by the remote, and
    // the remote might expect the components insert to be received at the same time)
    pub(crate) fn prepare_component_insert(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        component: RawData,
    ) {
        // if self
        //     .pending_unique_components
        //     .entry(group_id)
        //     .or_default()
        //     .entry(entity)
        //     .or_default()
        //     .contains(&kind)
        // {
        //     debug!(
        //         ?group_id,
        //         ?entity,
        //         ?kind,
        //         "Trying to insert a component that is already in the message"
        //     );
        //     return;
        // }
        self.pending_actions
            .entry(group_id)
            .or_default()
            .entry(entity)
            .or_default()
            .insert
            .push(component);
        // self.pending_unique_components
        //     .entry(group_id)
        //     .or_default()
        //     .entry(entity)
        //     .or_default()
        //     .insert(kind);
    }

    pub(crate) fn prepare_component_remove(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        kind: ComponentNetId,
    ) {
        // TODO: is the pending_unique_components even necessary? how could we even happen multiple inserts/updates for the same component?
        // if self
        //     .pending_unique_components
        //     .entry(group_id)
        //     .or_default()
        //     .entry(entity)
        //     .or_default()
        //     .contains(&kind)
        // {
        //     error!(
        //         ?group_id,
        //         ?entity,
        //         ?kind,
        //         "Trying to remove a component that is already in the message as an insert/update"
        //     );
        //     return;
        // }
        self.pending_actions
            .entry(group_id)
            .or_default()
            .entry(entity)
            .or_default()
            .remove
            .insert(kind);
    }

    pub(crate) fn prepare_component_update(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        raw_data: RawData,
    ) {
        self.pending_updates
            .entry(group_id)
            .or_default()
            .entry(entity)
            .or_default()
            .push(raw_data);
    }

    /// Create a component update.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_delta_component_update(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        kind: ComponentKind,
        component_data: Ptr,
        registry: &ComponentRegistry,
        writer: &mut BitcodeWriter,
        tick: Tick,
    ) {
        let group_channel = self.group_channels.entry(group_id).or_default();

        let entry = group_channel
            .delta_component_values
            .entry(kind)
            .or_default();

        // Serialize the data (either by computing the diff with a previously acked value, or by serializing the base value)
        let raw_data = entry
            .acked
            .then(|| {
                // get the last previously sent value for this component that was acked
                // the oldest value is the one that was acked (since we keep removing all the items
                // before the last one acked)
                // we do not want to pop because the value might be used for future diffs
                entry
                    .component_data
                    .first_entry()
                    .and_then(|item| {
                        item.get().get(&entity).and_then(|erased_data| {
                            Some(
                                // SAFETY: the component_data and erased_data is a pointer to a component that corresponds to kind
                                unsafe {
                                    registry
                                        .serialize_diff(
                                            Ptr::new(*erased_data),
                                            component_data,
                                            writer,
                                            kind,
                                        )
                                        .expect("could not serialize delta")
                                },
                            )
                        })
                    })
                    .unwrap_or_else(|| {
                        // SAFETY: the component_data is a pointer to a component that corresponds to kind
                        unsafe {
                            // compute a diff from the base value, and serialize that
                            registry
                                .serialize_diff_from_base_value(component_data, writer, kind)
                                .expect("could not serialize delta")
                        }
                    })
            })
            .unwrap_or_else(|| {
                // SAFETY: the component_data is a pointer to a component that corresponds to kind
                unsafe {
                    // compute a diff from the base value, and serialize that
                    registry
                        .serialize_diff_from_base_value(component_data, writer, kind)
                        .expect("could not serialize delta")
                }
            });

        // Store the component data so that we can compute future diffs
        // SAFETY: the component_data corresponds to the kind
        let cloned_data = unsafe {
            registry
                .erased_clone(component_data, kind)
                .expect("could not clone component data")
        };
        entry
            .component_data
            .entry(tick)
            .or_default()
            .insert(entity, cloned_data);

        trace!(?kind, "Inserting pending update!");
        self.pending_updates
            .entry(group_id)
            .or_default()
            .entry(entity)
            .or_default()
            .push(raw_data);
        // self.pending_unique_components
        //     .entry(group_id)
        //     .or_default()
        //     .entry(entity)
        //     .or_default()
        //     .insert(kind);
    }

    /// Finalize the replication messages
    pub(crate) fn finalize(
        &mut self,
        tick: Tick,
    ) -> Vec<(ChannelKind, ReplicationGroupId, ReplicationMessageData, f32)> {
        let mut messages = Vec::new();

        for (group_id, mut actions) in self.pending_actions.drain() {
            trace!(?group_id, "pending actions: {:?}", actions);
            // add any updates for that group
            if let Some(updates) = self.pending_updates.remove(&group_id) {
                trace!(?group_id, "found updates for group: {:?}", updates);
                for (entity, components) in updates {
                    actions
                        .entry(entity)
                        .or_default()
                        .updates
                        .extend(components.into_iter());
                }
            }
            let channel = self.group_channels.entry(group_id).or_default();
            let priority = channel
                .accumulated_priority
                .unwrap_or(channel.base_priority);
            let message_id = channel.actions_next_send_message_id;
            channel.actions_next_send_message_id += 1;
            channel.last_action_tick = Some(tick);
            messages.push((
                ChannelKind::of::<EntityActionsChannel>(),
                group_id,
                ReplicationMessageData::Actions(EntityActionMessage {
                    sequence_id: message_id,
                    // TODO: maybe we can just send the HashMap directly?
                    actions: Vec::from_iter(actions.into_iter()),
                }),
                priority,
            ));
            debug!("final action messages to send: {:?}", messages);
        }
        // send the remaining updates
        for (group_id, updates) in self.pending_updates.drain() {
            trace!(?group_id, "pending updates: {:?}", updates);
            let channel = self.group_channels.entry(group_id).or_default();
            let priority = channel
                .accumulated_priority
                .unwrap_or(channel.base_priority);
            messages.push((
                ChannelKind::of::<EntityUpdatesChannel>(),
                group_id,
                ReplicationMessageData::Updates(EntityUpdatesMessage {
                    // SAFETY: the last action tick is always set because we send Actions before Updates
                    last_action_tick: channel.last_action_tick,
                    // TODO: maybe we can just send the HashMap directly?
                    updates: Vec::from_iter(updates.into_iter()),
                }),
                priority,
            ));
        }

        if !messages.is_empty() {
            debug!(?messages, "Sending replication messages");
        }

        // clear send buffers
        // self.pending_unique_components.clear();

        // TODO: also return for each message a list of the components that have diff data
        messages
    }
}

/// Type-erased component data that we store for delta compression
///
/// We keep in memory the value of some past components so that we can compute delta-compression
/// between the last acked value and the current value.
#[derive(Debug, Default)]
struct DeltaComponentSentData {
    /// Is true if the value for the lowest tick was acked by the client.
    ///
    /// At the start it's false, and then it stays true all the time, since we will keep doing diffs
    /// with the last acked value.
    // TODO: may be false with tick-clamping
    acked: bool,
    // TODO: use any_vec?
    /// NonNull<u8> is the type-erased owned component data
    component_data: BTreeMap<Tick, EntityHashMap<Entity, NonNull<u8>>>,
}

unsafe impl Send for DeltaComponentSentData {}
unsafe impl Sync for DeltaComponentSentData {}

/// Channel to keep track of sending replication messages for a given Group
#[derive(Debug)]
pub struct GroupChannel {
    /// For the components that have delta compression enabled, we need to store in memory
    /// the last value that was acked by the client, so that we can send the delta between the last acked value
    /// This is a map from the
    // TODO: maybe use a BlobVec since we want to store type-erased data?
    pub delta_component_values: HashMap<ComponentKind, DeltaComponentSentData>,
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
    pub ack_tick: Option<BevyTick>,

    // last tick for which we sent an action message
    pub last_action_tick: Option<Tick>,

    /// The priority to send the replication group.
    /// This will be reset to base_priority every time we send network updates, unless we couldn't send a message
    /// for this group because of the bandwidth cap, in which case it will be accumulated.
    pub accumulated_priority: Option<f32>,
    pub base_priority: f32,
}

impl Default for GroupChannel {
    fn default() -> Self {
        Self {
            delta_component_values: HashMap::default(),
            actions_next_send_message_id: MessageId(0),
            send_tick: None,
            ack_tick: None,
            last_action_tick: None,
            accumulated_priority: None,
            base_priority: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::channel::builder::EntityActionsChannel;
    use crate::prelude::server::Replicate;
    use crate::prelude::{ClientId, ReplicationGroup};
    use crate::server::connection::ConnectionManager;
    use crate::shared::replication::components::ReplicationGroupIdBuilder;
    use crate::tests::protocol::Component1;
    use crate::tests::stepper::{BevyStepper, Step, TEST_CLIENT_ID};
    use bevy::prelude::*;

    use super::*;

    /// Test that if we receive a nack, we bump the send_tick down to the ack tick
    #[test]
    fn test_integration_send_tick_updates_on_packet_nack() {
        let mut stepper = BevyStepper::default();
        macro_rules! sender {
            () => {
                stepper
                    .server_app
                    .world
                    .resource::<ConnectionManager>()
                    .connections
                    .get(&ClientId::Netcode(TEST_CLIENT_ID))
                    .unwrap()
                    .replication_sender
            };
        }
        let server_entity = stepper
            .server_app
            .world
            .spawn((Component1(1.0), Replicate::default()))
            .id();
        stepper.frame_step();

        // send an update
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .get_mut::<Component1>()
            .unwrap()
            .0 = 2.0;
        stepper.frame_step();
        let server_tick = stepper.server_tick();

        // check that we keep track of the message_id and bevy_tick at which we sent the update
        let (
            message_id,
            UpdateMessageMetadata {
                group_id,
                bevy_tick,
                ..
            },
        ) = sender!()
            .updates_message_id_to_group_id
            .iter()
            .next()
            .expect("we should have stored the message_id associated with the entity update");
        assert_eq!(group_id, &ReplicationGroupId(server_entity.to_bits()));
        let group_channel = sender!()
            .group_channels
            .get(group_id)
            .expect("we should have a group channel for the entity");
        assert_eq!(group_channel.send_tick, Some(*bevy_tick));
        assert_eq!(group_channel.ack_tick, None);
    }

    #[test]
    fn test_send_tick_no_priority() {
        // create fake channels for receiving updates about acks and sends
        let component_registry = ComponentRegistry::default();
        let (tx_ack, rx_ack) = crossbeam_channel::unbounded();
        let (tx_nack, rx_nack) = crossbeam_channel::unbounded();
        let (tx_send, rx_send) = crossbeam_channel::unbounded();
        let mut sender = ReplicationSender::new(rx_ack, rx_nack, rx_send, false, false);
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
        sender.buffer_replication_update_message(group_1, message_1, bevy_tick_1, tick_1);
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_1),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_1,
                tick: tick_1
            })
        );
        assert_eq!(group.send_tick, Some(bevy_tick_1));
        assert_eq!(group.ack_tick, None);

        // if we buffer a second message, we update the `send_tick`
        sender.buffer_replication_update_message(group_1, message_2, bevy_tick_2, tick_2);
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_2),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_2,
                tick: tick_2
            })
        );
        assert_eq!(group.send_tick, Some(bevy_tick_2));
        assert_eq!(group.ack_tick, None);

        // if we receive an ack for the second message, we update the `ack_tick`
        tx_ack.try_send(message_2).unwrap();
        sender.recv_update_acks(&component_registry);
        let group = sender.group_channels.get(&group_1).unwrap();
        assert!(!sender
            .updates_message_id_to_group_id
            .contains_key(&message_2));
        assert_eq!(group.send_tick, Some(bevy_tick_2));
        assert_eq!(group.ack_tick, Some(bevy_tick_2));

        // if we buffer a third message, we update the `send_tick`
        sender.buffer_replication_update_message(group_1, message_3, bevy_tick_3, tick_3);
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_3),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_3,
                tick: tick_3
            })
        );
        assert_eq!(group.send_tick, Some(bevy_tick_3));
        assert_eq!(group.ack_tick, Some(bevy_tick_2));

        // if we receive a nack for the first message, we don't care because that message's bevy tick
        // is lower than our ack tick
        tx_nack.try_send(message_1).unwrap();
        sender.update(BevyTick::new(10));
        // make sure that the send tick wasn't updated
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(group.send_tick, Some(bevy_tick_3));

        // however if we receive a nack for the third message, we update the `send_tick` back to the `ack_tick`
        tx_nack.try_send(message_3).unwrap();
        sender.update(BevyTick::new(10));
        let group = sender.group_channels.get(&group_1).unwrap();
        assert!(!sender
            .updates_message_id_to_group_id
            .contains_key(&message_3),);
        // this time the `send_tick` is updated to the `ack_tick`
        assert_eq!(group.send_tick, Some(bevy_tick_2));
        assert_eq!(group.ack_tick, Some(bevy_tick_2));
    }

    #[test]
    fn test_send_tick_priority() {
        // create fake channels for receiving updates about acks and sends
        let (tx_ack, rx_ack) = crossbeam_channel::unbounded();
        let (tx_nack, rx_nack) = crossbeam_channel::unbounded();
        let (tx_send, rx_send) = crossbeam_channel::unbounded();
        let mut sender = ReplicationSender::new(rx_ack, rx_nack, rx_send, false, true);
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
        // when we buffer a message to be sent, we don't update the `send_tick`
        // (because we wait until the message is actually send)
        sender.buffer_replication_update_message(group_1, message_1, bevy_tick_1, tick_1);
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(
            sender.updates_message_id_to_group_id.get(&message_1),
            Some(&UpdateMessageMetadata {
                group_id: group_1,
                bevy_tick: bevy_tick_1,
                tick: tick_1
            })
        );
        assert_eq!(group.send_tick, None);
        assert_eq!(group.ack_tick, None);

        tx_send.try_send(message_1).unwrap();
        // when the message is actually sent, we update the `send_tick`
        sender.recv_send_notification();
        let group = sender.group_channels.get(&group_1).unwrap();
        assert_eq!(group.send_tick, Some(bevy_tick_1));
        assert_eq!(group.ack_tick, None);
    }

    // TODO: add tests for replication with entity relations!
    /// Test calling the `finalize` method to create the final replication messages
    /// from the buffered actions and updates
    #[test]
    fn test_finalize() {
        // create fake channels for receiving updates about acks and sends
        let (tx_ack, rx_ack) = crossbeam_channel::unbounded();
        let (tx_nack, rx_nack) = crossbeam_channel::unbounded();
        let (tx_send, rx_send) = crossbeam_channel::unbounded();
        let mut manager = ReplicationSender::new(rx_ack, rx_nack, rx_send, false, false);

        let entity_1 = Entity::from_raw(0);
        let entity_2 = Entity::from_raw(1);
        let entity_3 = Entity::from_raw(2);
        let group_1 = ReplicationGroupId(0);
        let group_2 = ReplicationGroupId(1);
        let net_id_1: ComponentNetId = 0;
        let net_id_2: ComponentNetId = 1;
        let net_id_3: ComponentNetId = 1;
        let raw_1 = vec![0];
        let raw_2 = vec![1];
        let raw_3 = vec![2];
        let raw_4 = vec![3];

        manager.group_channels.insert(
            group_1,
            GroupChannel {
                actions_next_send_message_id: MessageId(2),
                ..Default::default()
            },
        );
        manager.group_channels.insert(
            group_2,
            GroupChannel {
                last_action_tick: Some(Tick(3)),
                ..Default::default()
            },
        );

        // updates should be grouped with actions
        manager.prepare_entity_spawn(entity_1, group_1);
        manager.prepare_component_insert(entity_1, group_1, raw_1.clone());
        manager.prepare_component_remove(entity_1, group_1, net_id_2);
        manager.prepare_component_update(entity_1, group_1, raw_2.clone());

        // handle another entity in the same group: will be added to EntityActions as well
        manager.prepare_component_update(entity_2, group_1, raw_3.clone());

        manager.prepare_component_update(entity_3, group_2, raw_4.clone());

        // the order of actions is not important if there are no relations between the entities
        let message = manager.finalize(Tick(2));
        let actions = message.first().unwrap();
        assert_eq!(actions.0, ChannelKind::of::<EntityActionsChannel>());
        assert_eq!(actions.1, group_1);
        let ReplicationMessageData::Actions(ref a) = actions.2 else {
            panic!()
        };
        assert_eq!(a.sequence_id, MessageId(2));
        assert_eq!(
            EntityHashMap::from_iter(a.actions.clone()),
            EntityHashMap::from_iter(vec![
                (
                    entity_1,
                    EntityActions {
                        spawn: SpawnAction::Spawn,
                        insert: vec![raw_1],
                        remove: HashSet::from_iter(vec![net_id_2]),
                        updates: vec![raw_2],
                    }
                ),
                (
                    entity_2,
                    EntityActions {
                        spawn: SpawnAction::None,
                        insert: vec![],
                        remove: HashSet::default(),
                        updates: vec![raw_3],
                    }
                )
            ])
        );

        let updates = message.get(1).unwrap();
        assert_eq!(
            updates,
            &(
                ChannelKind::of::<EntityUpdatesChannel>(),
                group_2,
                ReplicationMessageData::Updates(EntityUpdatesMessage {
                    last_action_tick: Some(Tick(3)),
                    updates: vec![(entity_3, vec![raw_4])],
                }),
                1.0
            )
        );
        assert_eq!(
            manager
                .group_channels
                .get(&group_1)
                .unwrap()
                .actions_next_send_message_id,
            MessageId(3)
        );
        assert_eq!(
            manager
                .group_channels
                .get(&group_1)
                .unwrap()
                .last_action_tick,
            Some(Tick(2))
        );
    }
}
