//! General struct handling replication
use anyhow::Context;
use bevy::a11y::accesskit::Action;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::_reexport::{EntityActionsChannel, EntityUpdatesChannel};
use crate::connection::events::ConnectionEvents;
use bevy::prelude::{Entity, EntityWorldMut, World};
use bevy::utils::EntityHashMap;
use crossbeam_channel::Receiver;
use tracing::{debug, error, info, trace, trace_span};
use tracing_subscriber::filter::FilterExt;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use super::entity_map::EntityMap;
use super::{
    EntityActionMessage, EntityActions, EntityUpdatesMessage, Replicate, ReplicationMessage,
    ReplicationMessageData,
};
use crate::connection::message::ProtocolMessage;
use crate::packet::message::MessageId;
use crate::prelude::Tick;
use crate::protocol::channel::ChannelKind;
use crate::protocol::component::{ComponentBehaviour, ComponentKindBehaviour};
use crate::protocol::Protocol;
use crate::shared::replication::components::{ReplicationGroup, ReplicationGroupId};

// TODO: maybe store additional information about the entity?
//  (e.g. the value of the replicate component)?
pub enum EntityStatus {
    JustSpawned,
    Spawning,
    Spawned,
}

pub type BevyTick = bevy::ecs::component::Tick;

pub(crate) struct ReplicationManager<P: Protocol> {
    pub remote_entity_status: HashMap<Entity, EntityStatus>,
    // pub global_replication_data: &'a ReplicationData,
    /// Map between local and remote entities. (used mostly on client because it's when we receive entity updates)
    pub entity_map: EntityMap,
    /// Buffer to so that we have an ordered receiver per group
    pub group_channels: EntityHashMap<ReplicationGroupId, GroupChannel<P>>,
    /// Get notified whenever a message-id that was sent has been received by the remote
    pub updates_ack_tracker: Receiver<MessageId>,
    /// Map from message-id to the corresponding group-id that sent this update message
    pub updates_message_id_to_group_id: HashMap<MessageId, ReplicationGroupId>,

    /// messages that are being written. We need to hold a buffer of messages because components actions/updates
    /// are being buffered individually but we want to group them inside a message
    pub pending_actions: EntityHashMap<
        ReplicationGroupId,
        BTreeMap<Entity, EntityActions<P::Components, P::ComponentKinds>>,
    >,
    pub pending_updates: EntityHashMap<ReplicationGroupId, BTreeMap<Entity, Vec<P::Components>>>,
}

/// Channel to keep track of receiving/sending replication messages for a given Group
pub struct GroupChannel<P: Protocol> {
    // actions
    pub actions_next_send_message_id: MessageId,
    pub actions_pending_recv_message_id: MessageId,
    pub actions_recv_message_buffer:
        BTreeMap<MessageId, (Tick, EntityActionMessage<P::Components, P::ComponentKinds>)>,
    // last tick for which we sent an action message
    pub last_action_tick: Tick,

    // updates
    // map from necessary_last_action_tick to the buffered message
    pub updates_waiting_for_insert:
        BTreeMap<Tick, BTreeMap<Tick, EntityUpdatesMessage<P::Components>>>,
    // list of update messages that we can apply immediately (in order)
    pub updates_ready_to_apply: Vec<(Tick, EntityUpdatesMessage<P::Components>)>,
    // TODO: maybe also keep track of which Tick this bevy-tick corresponds to? (will enable doing diff-compression)
    // TODO: maybe this should be an Option, so that we make sure that when we need it's always is_some()
    // bevy tick when we received an ack of an update for this group
    pub latest_updates_ack_bevy_tick: BevyTick,

    // both
    /// last server tick that we applied to the client world
    pub latest_tick: Tick,
}

impl<P: Protocol> Default for GroupChannel<P> {
    fn default() -> Self {
        Self {
            actions_next_send_message_id: MessageId(0),
            actions_pending_recv_message_id: MessageId(0),
            actions_recv_message_buffer: BTreeMap::new(),
            last_action_tick: Tick(0),
            updates_waiting_for_insert: Default::default(),
            updates_ready_to_apply: vec![],
            latest_updates_ack_bevy_tick: BevyTick::new(0),
            latest_tick: Tick(0),
        }
    }
}

impl<P: Protocol> GroupChannel<P> {
    /// Reads a message from the internal buffer to get its content
    /// Since we are receiving messages in order, we don't return from the buffer
    /// until we have received the message we are waiting for (the next expected MessageId)
    /// This assumes that the sender sends all message ids sequentially.
    ///
    /// If had received updates that were waiting on a given action, we also return them
    fn read_action(
        &mut self,
    ) -> Option<(Tick, EntityActionMessage<P::Components, P::ComponentKinds>)> {
        // Check if we have received the message we are waiting for
        let Some(message) = self
            .actions_recv_message_buffer
            .remove(&self.actions_pending_recv_message_id)
        else {
            return None;
        };

        self.actions_pending_recv_message_id += 1;
        // Update the latest server tick that we have processed
        self.latest_tick = message.0;
        Some(message)
    }

    /// Return the update that we are ready to apply now
    fn read_update(&mut self) -> Vec<(Tick, EntityUpdatesMessage<P::Components>)> {
        std::mem::take(&mut self.updates_ready_to_apply)
    }

    fn read_buffered_updates(&mut self) -> Vec<(Tick, EntityUpdatesMessage<P::Components>)> {
        // go through all the buffered updates whose last_action_tick has been reached
        let not_ready = self.updates_waiting_for_insert.split_off(&self.latest_tick);

        let mut res = vec![];
        for (_, updates) in self.updates_ready_to_apply.into_iter() {
            for (tick, update) in updates {
                // if we have applied a more recent tick, just discard the update
                if tick > self.latest_tick {
                    self.latest_tick = tick;
                    res.push((update.last_action_tick, update.clone()));
                }
            }
        }
        std::mem::replace(&mut self.updates_waiting_for_insert, not_ready);
        res
    }
}

impl<P: Protocol> ReplicationManager<P> {
    fn new(updates_ack_tracker: Receiver<MessageId>) -> Self {
        Self {
            entity_map: EntityMap::default(),
            remote_entity_status: HashMap::new(),
            pending_actions: EntityHashMap::default(),
            pending_updates: EntityHashMap::default(),
            // global_replication_data,
            group_channels: Default::default(),
            updates_ack_tracker,
            updates_message_id_to_group_id: Default::default(),
        }
    }

    // TODO: call this in a system after receive
    /// We call this after receive stage; to update the bevy_tick at which we received entity udpates for each group
    pub(crate) fn recv_update_acks(&mut self, bevy_tick: bevy::ecs::component::Tick) {
        // TODO: handle errors that are not channel::isEmpty
        while let Ok(message_id) = self.updates_ack_tracker.try_recv() {
            if let Some(group_id) = self.updates_message_id_to_group_id.get(&message_id) {
                let channel = self.group_channels.entry(*group_id).or_default();
                // TODO: doesn't seem like we need to do a MAX
                channel.latest_updates_ack_bevy_tick =
                    std::cmp::max(channel.latest_updates_ack_bevy_tick, bevy_tick);
            } else {
                error!(
                    "Received an update message-id ack but we don't have the corresponding group"
                );
            }
        }
        // TODO: should we do the same thing for self.actions_ack_tracker?
    }

    /// Recv a new replication message and buffer it
    pub(crate) fn recv_message(
        &mut self,
        message: ReplicationMessage<P::Components, P::ComponentKinds>,
        tick: Tick,
    ) {
        let channel = self.group_channels.entry(message.group_id).or_default();
        match message.data {
            ReplicationMessageData::Actions(m) => {
                // if the message is too old, ignore it
                if m.sequence_id < channel.actions_pending_recv_message_id {
                    return;
                }

                // add the message to the buffer
                // TODO: I guess this handles potential duplicates?
                channel
                    .actions_recv_message_buffer
                    .insert(m.sequence_id, (tick, m));
            }
            ReplicationMessageData::Updates(m) => {
                // TODO: instead of m.last_action_tick, we could include m.last_ack_tick?
                // if we haven't applied the required actions tick, buffer the updates
                if channel.latest_tick <= m.last_action_tick {
                    channel
                        .updates_waiting_for_insert
                        .entry(m.last_action_tick)
                        .or_default()
                        .entry(tick)
                        .or_insert(m);
                    return;
                }
                // if we have already applied a more recent update for this group, no need to keep this one
                if tick <= channel.latest_tick {
                    return;
                }

                // update is ready to be applied immediately!
                channel.latest_tick = tick;
                channel.updates_ready_to_apply.push((tick, m));
            }
        }
    }

    /// Return the list of replication messages that are ready to be applied to the World
    /// Updates the `latest_tick` for this group
    pub(crate) fn read_messages(
        &mut self,
    ) -> impl Iterator<
        Item = (
            ReplicationGroup,
            Vec<ReplicationMessageData<P::Components, P::ComponentKinds>>,
        ),
    > {
        self.group_channels.iter_mut().map(|(group_id, channel)| {
            let mut res = Vec::new();
            // check for any actions that are ready to be applied
            while let Some((tick, actions)) = channel.read_action() {
                res.push(ReplicationMessageData::Actions(actions));
            }

            // check for any updates that are ready to be applied
            res.extend(channel.read_update());

            // check for any buffered updates that are ready to be applied now that we have applied more actions/updates
            res.extend(channel.read_buffered_updates());

            (group_id, res)
        })
    }
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
///
/// - all component inserts/removes/updates for an entity to be grouped together in a single message
impl<P: Protocol> ReplicationManager<P> {
    // TODO: how can I emit metrics here that contain the channel kind?
    //  use a OnceCell that gets set with the channel name mapping when the protocol is finalized?
    //  the other option is to have wrappers in Connection, but that's pretty ugly

    /// Host has spawned an entity, and we want to replicate this to remote
    /// Returns true if we should send a message
    pub(crate) fn prepare_entity_spawn(&mut self, entity: Entity, group: ReplicationGroupId) {
        let mut actions = self
            .pending_actions
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default();
        actions.spawn = true;
    }

    pub(crate) fn prepare_entity_despawn(&mut self, entity: Entity, group: ReplicationGroupId) {
        self.pending_actions
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .despawn = true;
    }

    // we want to send all component inserts that happen together for the same entity in a single message
    // (because otherwise the inserts might be received at different packets/ticks by the remote, and
    // the remote might expect the components insert to be received at the same time)
    pub(crate) fn prepare_component_insert(
        &mut self,
        entity: Entity,
        group: ReplicationGroupId,
        component: P::Components,
    ) {
        self.pending_actions
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .insert
            .push(component);
    }

    pub(crate) fn prepare_component_remove(
        &mut self,
        entity: Entity,
        group: ReplicationGroupId,
        component: P::ComponentKinds,
    ) {
        self.pending_actions
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .remove
            .push(component);
    }

    pub(crate) fn prepare_entity_update(
        &mut self,
        entity: Entity,
        group: ReplicationGroupId,
        component: P::Components,
    ) {
        self.pending_updates
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .push(component);
    }

    /// Finalize the replication messages
    pub(crate) fn finalize(
        &mut self,
        tick: Tick,
    ) -> Vec<(
        ChannelKind,
        ReplicationGroupId,
        ReplicationMessageData<P::Components, P::ComponentKinds>,
    )> {
        let mut messages = Vec::new();

        // if there are any entity actions, send EntityActions
        for (group_id, mut actions) in self.pending_actions.drain() {
            // add any updates for that group
            if let Some(updates) = self.pending_updates.remove(&group_id) {
                for (entity, components) in updates {
                    actions
                        .entry(entity)
                        .or_default()
                        .updates
                        .extend(components);
                }
            }
            let channel = self.group_channels.entry(group_id).or_default();
            let message_id = channel.actions_next_send_message_id;
            channel.actions_next_send_message_id += 1;
            channel.last_action_tick = tick;
            messages.push((
                ChannelKind::of::<EntityActionsChannel>(),
                group_id,
                ReplicationMessageData::Actions(EntityActionMessage {
                    sequence_id: message_id,
                    actions,
                }),
            ));
        }

        // send the remaining updates
        for (group_id, updates) in self.pending_updates.drain() {
            let channel = self.group_channels.entry(group_id).or_default();
            messages.push((
                ChannelKind::of::<EntityUpdatesChannel>(),
                group_id,
                ReplicationMessageData::Updates(EntityUpdatesMessage {
                    last_action_tick: channel.last_action_tick,
                    updates,
                }),
            ));
        }

        messages
    }

    /// Apply any replication messages to the world, and emit an event
    /// I think we don't need to emit a tick with the event anymore, because
    /// we can access the tick via the replication manager
    pub(crate) fn apply_world(
        &mut self,
        world: &mut World,
        replication: ReplicationMessageData<P::Components, P::ComponentKinds>,
        events: &mut ConnectionEvents<P>,
    ) {
        let _span = trace_span!("Apply received replication message to world").entered();
        match replication {
            ReplicationMessageData::Actions(m) => {
                for (entity, actions) in m.actions.into_iter() {
                    debug!(remote_entity = ?entity, "Received entity actions");

                    // spawn
                    let mut local_entity: EntityWorldMut;
                    if actions.spawn {
                        // TODO: optimization: spawn the bundle of insert components
                        local_entity = world.spawn_empty();
                        self.entity_map.insert(entity, local_entity.id());

                        debug!(remote_entity = ?entity, "Received entity spawn");
                        events.push_spawn(local_entity.id());
                    } else {
                        if let Ok(l) = self.entity_map.get_by_remote(world, entity) {
                            local_entity = l;
                        } else {
                            continue;
                        }
                    }

                    // despawn
                    if actions.despawn {
                        debug!(remote_entity = ?entity, "Received entity despawn");
                        if let Some(local_entity) = self.entity_map.remove_by_remote(entity) {
                            events.push_despawn(local_entity);
                            world.despawn(local_entity);
                        } else {
                            error!("Received despawn for an entity that does not exist")
                        }
                    }
                    // inserts
                    let kinds = actions
                        .insert
                        .iter()
                        .map(|c| c.into())
                        .collect::<Vec<P::ComponentKinds>>();
                    debug!(remote_entity = ?entity, ?kinds, "Received InsertComponent");
                    for component in actions.insert {
                        // TODO: figure out what to do with tick here
                        events.push_insert_component(local_entity.id(), component.into(), Tick(0));
                        component.insert(&mut local_entity);
                    }

                    // removals
                    debug!(remote_entity = ?entity, ?actions.remove, "Received RemoveComponent");
                    for kind in actions.remove {
                        events.push_remove_component(local_entity.id(), kind, Tick(0));
                        kind.remove(&mut local_entity);
                    }

                    // (no need to run apply_deferred after applying actions, that is only for Commands)

                    // updates
                    let kinds = actions
                        .updates
                        .iter()
                        .map(|c| c.into())
                        .collect::<Vec<P::ComponentKinds>>();
                    debug!(remote_entity = ?entity, ?kinds, "Received UpdateComponent");
                    for component in actions.updates {
                        events.push_update_component(local_entity.id(), component.into(), Tick(0));
                        component.update(&mut local_entity);
                    }
                }
            }
            ReplicationMessageData::Updates(m) => {
                for (entity, components) in m.updates.into_iter() {
                    debug!(remote_entity = ?entity, "Received entity updates");
                    let kinds = components
                        .iter()
                        .map(|c| c.into())
                        .collect::<Vec<P::ComponentKinds>>();
                    debug!(?entity, ?kinds, "Received UpdateComponent");
                    // if the entity does not exist, create it
                    if let Ok(mut local_entity) = self.entity_map.get_by_remote(world, entity) {
                        for component in components {
                            events.push_update_component(
                                local_entity.id(),
                                component.into(),
                                Tick(0),
                            );
                            component.update(&mut local_entity);
                        }
                    } else {
                        // the entity has been despawned by one of the previous actions
                        // still, is this possible? we should only receive updates that are after the despawn...
                        error!("update for entity that doesn't exist?");
                    }
                }
            }
        }
    }

    // /// Apply any replication messages to the world
    // pub(crate) fn apply_world(
    //     &mut self,
    //     world: &mut World,
    //     replication: ReplicationMessage<P::Components, P::ComponentKinds>,
    // ) {
    //     let _span = trace_span!("Apply received replication message to world").entered();
    //     match replication {
    //         ReplicationMessage::SpawnEntity(entity, components) => {
    //             let component_kinds = components
    //                 .iter()
    //                 .map(|c| c.into())
    //                 .collect::<Vec<P::ComponentKinds>>();
    //             debug!(remote_entity = ?entity, ?component_kinds, "Received spawn entity");
    //
    //             // TODO: we only run spawn_entity if we don't already have an entity in the process of being spawned
    //             //  so we need a data-structure to keep track of entities that are being spawned
    //             //  or do we? I'm not sure we would send this twice, because of the bevy system logic
    //             //  but maybe we would do, if we remove Replicate and then Re-add it?
    //
    //             // Ignore if we already received the entity
    //             if self.entity_map.get_local(entity).is_some() {
    //                 return;
    //             }
    //             let mut local_entity_mut = world.spawn_empty();
    //
    //             // TODO: optimize by using batch functions?
    //             for component in components {
    //                 component.insert(&mut local_entity_mut);
    //             }
    //             self.entity_map.insert(entity, local_entity_mut.id());
    //         }
    //         ReplicationMessage::DespawnEntity(entity) => {
    //             // TODO: we only run this if the entity has been confirmed to be spawned on client?
    //             //  or should we send the message right away and let the receiver handle the ordering?
    //             //  (what if they receive despawn before spawn?)
    //             if let Some(local_entity) = self.entity_map.remove_by_remote(entity) {
    //                 world.despawn(local_entity);
    //             }
    //         }
    //         ReplicationMessage::InsertComponent(entity, components) => {
    //             let kinds = components
    //                 .iter()
    //                 .map(|c| c.into())
    //                 .collect::<Vec<P::ComponentKinds>>();
    //             debug!(remote_entity = ?entity, ?kinds, "Received InsertComponent");
    //             // it's possible that we received InsertComponent before the entity actually exists.
    //             // In that case, we need to spawn the entity first.
    //             // TODO: this might not be what we want? imagine we receive a DespawnEntity or RemoveComponent right before that?
    //             let mut local_entity_mut = self.entity_map.get_by_remote_or_spawn(world, entity);
    //             // TODO: maybe check if the component already exists?
    //             for component in components {
    //                 component.insert(&mut local_entity_mut);
    //             }
    //         }
    //         ReplicationMessage::RemoveComponent(entity, component_kinds) => {
    //             debug!(remote_entity = ?entity, kinds = ?component_kinds, "Received RemoveComponent");
    //             if let Some(local_entity) = self.entity_map.get_local(entity) {
    //                 if let Some(mut entity_mut) = world.get_entity_mut(*local_entity) {
    //                     for kind in component_kinds {
    //                         kind.remove(&mut entity_mut);
    //                     }
    //                 } else {
    //                     debug!(
    //                         "Could not remove component because local entity {:?} was not found",
    //                         local_entity
    //                     );
    //                 }
    //             }
    //             debug!(
    //                 "Could not remove component because remote entity {:?} was not found",
    //                 entity
    //             );
    //         }
    //         ReplicationMessage::EntityUpdate(entity, components) => {
    //             let kinds = components
    //                 .iter()
    //                 .map(|c| c.into())
    //                 .collect::<Vec<P::ComponentKinds>>();
    //             trace!(?entity, ?kinds, "Received entity update");
    //             // if the entity does not exist, create it
    //             let mut local_entity_mut = self.entity_map.get_by_remote_or_spawn(world, entity);
    //             // TODO: keep track of the components inserted?
    //             for component in components {
    //                 component.update(&mut local_entity_mut);
    //             }
    //         }
    //     }
    // }

    // pub fn buffer_spawn_entity<C: Channel>(&mut self, entity: Entity) {
    //     let message = MessageContainer::new(ReplicationMessage::SpawnEntity(entity));
    //     self.message_manager.buffer_send::<C>(message);
    // }
}
