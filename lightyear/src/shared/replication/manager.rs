//! General struct handling replication
use std::collections::BTreeMap;
use std::iter::Extend;

use anyhow::Context;
use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::{Entity, World};
use bevy::utils::petgraph::data::ElementIterator;
use bevy::utils::{EntityHashMap, HashMap, HashSet};
use crossbeam_channel::Receiver;
use tracing::{debug, error, info, trace, trace_span, warn};
use tracing_subscriber::filter::FilterExt;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::_reexport::{EntityActionsChannel, EntityUpdatesChannel, IntoKind};
use crate::connection::events::ConnectionEvents;
use crate::packet::message::MessageId;
use crate::prelude::{MapEntities, Tick};
use crate::protocol::channel::ChannelKind;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::component::{ComponentBehaviour, ComponentKindBehaviour};
use crate::protocol::Protocol;
use crate::shared::replication::components::ReplicationGroupId;

use super::entity_map::{InterpolatedEntityMap, PredictedEntityMap, RemoteEntityMap};
use super::{
    EntityActionMessage, EntityActions, EntityUpdatesMessage, ReplicationMessage,
    ReplicationMessageData,
};

// TODO: maybe separate send/receive side for clarity?
pub(crate) struct ReplicationManager<P: Protocol> {
    // pub global_replication_data: &'a ReplicationData,

    // SEND
    /// Get notified whenever a message-id that was sent has been received by the remote
    pub updates_ack_tracker: Receiver<MessageId>,
    /// Map from message-id to the corresponding group-id that sent this update message
    pub updates_message_id_to_group_id: HashMap<MessageId, ReplicationGroupId>,
    /// messages that are being written. We need to hold a buffer of messages because components actions/updates
    /// are being buffered individually but we want to group them inside a message
    pub pending_actions: EntityHashMap<
        ReplicationGroupId,
        HashMap<Entity, EntityActions<P::Components, P::ComponentKinds>>,
    >,
    pub pending_updates: EntityHashMap<ReplicationGroupId, HashMap<Entity, Vec<P::Components>>>,
    // Set of unique components for each entity, to avoid sending multiple updates/inserts for the same component
    pub pending_unique_components:
        EntityHashMap<ReplicationGroupId, HashMap<Entity, HashSet<P::ComponentKinds>>>,

    // RECEIVE
    /// Map between local and remote entities. (used mostly on client because it's when we receive entity updates)
    pub remote_entity_map: RemoteEntityMap,
    /// Map between remote and predicted entities
    pub predicted_entity_map: PredictedEntityMap,
    /// Map between remote and interpolated entities
    pub interpolated_entity_map: InterpolatedEntityMap,

    /// Map from remote entity to the replication group-id
    pub remote_entity_to_group: EntityHashMap<Entity, ReplicationGroupId>,

    // BOTH
    /// Buffer to so that we have an ordered receiver per group
    pub group_channels: EntityHashMap<ReplicationGroupId, GroupChannel<P>>,
}

impl<P: Protocol> ReplicationManager<P> {
    pub(crate) fn new(updates_ack_tracker: Receiver<MessageId>) -> Self {
        Self {
            // SEND
            updates_ack_tracker,
            updates_message_id_to_group_id: Default::default(),
            pending_actions: EntityHashMap::default(),
            pending_updates: EntityHashMap::default(),
            pending_unique_components: EntityHashMap::default(),
            // RECEIVE
            remote_entity_map: RemoteEntityMap::default(),
            predicted_entity_map: PredictedEntityMap::default(),
            interpolated_entity_map: InterpolatedEntityMap::default(),
            remote_entity_to_group: Default::default(),
            // BOTH
            group_channels: Default::default(),
        }
    }

    // TODO: call this in a system after receive
    /// We call this after receive stage; to update the bevy_tick at which we received entity udpates for each group
    pub(crate) fn recv_update_acks(&mut self, bevy_tick: bevy::ecs::component::Tick) {
        // TODO: handle errors that are not channel::isEmpty
        while let Ok(message_id) = self.updates_ack_tracker.try_recv() {
            if let Some(group_id) = self.updates_message_id_to_group_id.get(&message_id) {
                let channel = self.group_channels.entry(*group_id).or_default();
                channel.update_collect_changes_since_this_tick(bevy_tick);
            } else {
                error!(
                    "Received an update message-id ack but we don't have the corresponding group"
                );
            }
        }
    }

    /// Recv a new replication message and buffer it
    pub(crate) fn recv_message(
        &mut self,
        message: ReplicationMessage<P::Components, P::ComponentKinds>,
        tick: Tick,
    ) {
        trace!(?message, ?tick, "Received replication message");
        let channel = self.group_channels.entry(message.group_id).or_default();
        match message.data {
            ReplicationMessageData::Actions(m) => {
                // update the mapping from entity to group-id
                m.actions.iter().for_each(|(e, _)| {
                    self.remote_entity_to_group.insert(*e, message.group_id);
                });
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
                // update the mapping from entity to group-id
                m.updates.iter().for_each(|(e, _)| {
                    self.remote_entity_to_group.insert(*e, message.group_id);
                });
                // if we have already applied a more recent update for this group, no need to keep this one
                if tick <= channel.latest_tick {
                    return;
                }

                // TODO: include somewhere in the update message the m.last_ack_tick since when we compute changes?
                //  (if we want to do diff compression?
                // otherwise buffer the update
                channel
                    .buffered_updates
                    .entry(m.last_action_tick)
                    .or_default()
                    .entry(tick)
                    .or_insert(m);
            }
        }
        trace!(?channel, "group channel after buffering");
    }

    /// Return the list of replication messages that are ready to be applied to the World
    /// Also include the server_tick when that replication message was emitted
    ///
    /// Updates the `latest_tick` for this group
    pub(crate) fn read_messages(
        &mut self,
    ) -> Vec<(
        ReplicationGroupId,
        Vec<(
            Tick,
            ReplicationMessageData<P::Components, P::ComponentKinds>,
        )>,
    )> {
        self.group_channels
            .iter_mut()
            .filter_map(|(group_id, channel)| {
                let mut res = Vec::new();

                // check for any actions that are ready to be applied
                while let Some((tick, actions)) = channel.read_action() {
                    res.push((tick, ReplicationMessageData::Actions(actions)));
                }

                // TODO: (IMPORTANT): should we try to get the updates in order of tick?

                // check for any buffered updates that are ready to be applied now that we have applied more actions/updates
                res.extend(
                    channel
                        .read_buffered_updates()
                        .into_iter()
                        .map(|(tick, updates)| (tick, ReplicationMessageData::Updates(updates))),
                );

                (!res.is_empty()).then_some((*group_id, res))
            })
            .collect()
    }

    // USED BY RECEIVE SIDE (SEND SIZE CAN GET THE GROUP_ID EASILY)
    /// Get the group channel associated with a given entity
    pub(crate) fn channel_by_local(&self, local_entity: Entity) -> Option<&GroupChannel<P>> {
        self.remote_entity_map
            .get_remote(local_entity)
            .and_then(|remote_entity| self.channel_by_remote(*remote_entity))
    }

    // USED BY RECEIVE SIDE (SEND SIZE CAN GET THE GROUP_ID EASILY)
    /// Get the group channel associated with a given entity
    pub(crate) fn channel_by_remote(&self, remote_entity: Entity) -> Option<&GroupChannel<P>> {
        self.remote_entity_to_group
            .get(&remote_entity)
            .and_then(|group_id| self.group_channels.get(group_id))
    }
}

// TODO: handle duplicate inserts/updates/removes?
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
        let actions = self
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
        let kind: P::ComponentKinds = (&component).into();
        if self
            .pending_unique_components
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .contains(&kind)
        {
            warn!(
                ?group,
                ?entity,
                ?kind,
                "Trying to insert a component that is already in the message"
            );
            return;
        }
        self.pending_actions
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .insert
            .push(component);
        self.pending_unique_components
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .insert(kind);
    }

    pub(crate) fn prepare_component_remove(
        &mut self,
        entity: Entity,
        group: ReplicationGroupId,
        kind: P::ComponentKinds,
    ) {
        if self
            .pending_unique_components
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .contains(&kind)
        {
            error!(
                ?group,
                ?entity,
                ?kind,
                "Trying to remove a component that is already in the message as an insert/update"
            );
            return;
        }
        self.pending_actions
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .remove
            .insert(kind);
    }

    pub(crate) fn prepare_entity_update(
        &mut self,
        entity: Entity,
        group: ReplicationGroupId,
        component: P::Components,
    ) {
        let kind: P::ComponentKinds = (&component).into();
        if self
            .pending_unique_components
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .contains(&kind)
        {
            warn!(
                ?group,
                ?entity,
                ?kind,
                "Trying to update a component that is already in the message"
            );
            return;
        }
        self.pending_updates
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .push(component);
        self.pending_unique_components
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .insert(kind);
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

        // get the list of entities in topological order
        for (group_id, mut actions) in self.pending_actions.drain() {
            // add any updates for that group
            if let Some(updates) = self.pending_updates.remove(&group_id) {
                for (entity, components) in updates {
                    actions
                        .entry(entity)
                        .or_default()
                        .updates
                        .extend(components.into_iter());
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
                    // TODO: maybe we can just send the HashMap directly?
                    actions: Vec::from_iter(actions.into_iter()),
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
                    // TODO: maybe we can just send the HashMap directly?
                    updates: Vec::from_iter(updates.into_iter()),
                }),
            ));
        }

        if !messages.is_empty() {
            trace!(?messages, "Sending replication messages");
        }

        // clear send buffers
        self.pending_unique_components.clear();
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
                info!(?m, "Received replication actions");
                // NOTE: order matters here, because some components can depend on other entities.
                // These components could even form a cycle, for example A.HasWeapon(B) and B.HasHolder(A)
                // Our solution is to first handle spawn for all entities separately.
                for (entity, actions) in m.actions.iter() {
                    debug!(remote_entity = ?entity, "Received entity actions");
                    assert!(!(actions.spawn && actions.despawn));
                    // spawn
                    if actions.spawn {
                        // TODO: optimization: spawn the bundle of insert components
                        let local_entity = world.spawn_empty();
                        self.remote_entity_map.insert(*entity, local_entity.id());

                        debug!(remote_entity = ?entity, "Received entity spawn");
                        events.push_spawn(local_entity.id());
                    }
                }

                for (entity, actions) in m.actions.into_iter() {
                    info!(remote_entity = ?entity, "Received entity actions");

                    // despawn
                    if actions.despawn {
                        debug!(remote_entity = ?entity, "Received entity despawn");
                        if let Some(local_entity) = self.remote_entity_map.remove_by_remote(entity)
                        {
                            events.push_despawn(local_entity);
                            world.despawn(local_entity);
                        } else {
                            error!("Received despawn for an entity that does not exist")
                        }
                        continue;
                    }

                    // safety: we know by this point that the entity exists
                    let Ok(mut local_entity_mut) =
                        self.remote_entity_map.get_by_remote(world, entity)
                    else {
                        error!("cannot find entity");
                        continue;
                    };

                    // inserts
                    let kinds = actions
                        .insert
                        .iter()
                        .map(|c| c.into())
                        .collect::<HashSet<P::ComponentKinds>>();
                    info!(remote_entity = ?entity, ?kinds, "Received InsertComponent");
                    for mut component in actions.insert {
                        // map any entities inside the component
                        component.map_entities(Box::new(&self.remote_entity_map));
                        // TODO: figure out what to do with tick here
                        events.push_insert_component(
                            local_entity_mut.id(),
                            (&component).into(),
                            Tick(0),
                        );
                        component.insert(&mut local_entity_mut);
                    }

                    // removals
                    debug!(remote_entity = ?entity, ?actions.remove, "Received RemoveComponent");
                    for kind in actions.remove {
                        events.push_remove_component(local_entity_mut.id(), kind.clone(), Tick(0));
                        kind.remove(&mut local_entity_mut);
                    }

                    // (no need to run apply_deferred after applying actions, that is only for Commands)

                    // updates
                    let kinds = actions
                        .updates
                        .iter()
                        .map(|c| c.into())
                        .collect::<Vec<P::ComponentKinds>>();
                    debug!(remote_entity = ?entity, ?kinds, "Received UpdateComponent");
                    for mut component in actions.updates {
                        // map any entities inside the component
                        component.map_entities(Box::new(&self.remote_entity_map));
                        events.push_update_component(
                            local_entity_mut.id(),
                            (&component).into(),
                            Tick(0),
                        );
                        component.update(&mut local_entity_mut);
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
                    if let Ok(mut local_entity) =
                        self.remote_entity_map.get_by_remote(world, entity)
                    {
                        for component in components {
                            events.push_update_component(
                                local_entity.id(),
                                (&component).into(),
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
}

/// Channel to keep track of receiving/sending replication messages for a given Group
#[derive(Debug)]
pub struct GroupChannel<P: Protocol> {
    // SEND
    pub actions_next_send_message_id: MessageId,
    // TODO: maybe also keep track of which Tick this bevy-tick corresponds to? (will enable doing diff-compression)
    // TODO: maybe this should be an Option, so that we make sure that when we need it's always is_some()
    // bevy tick when we received an ack of an update for this group
    pub collect_changes_since_this_tick: BevyTick,
    // last tick for which we sent an action message
    pub last_action_tick: Tick,

    // RECEIVE
    pub actions_pending_recv_message_id: MessageId,
    pub actions_recv_message_buffer:
        BTreeMap<MessageId, (Tick, EntityActionMessage<P::Components, P::ComponentKinds>)>,
    // updates
    // map from necessary_last_action_tick to the buffered message
    // the first tick is the last_action_tick
    // the second tick is the update's server tick when it was sent
    pub buffered_updates: BTreeMap<Tick, BTreeMap<Tick, EntityUpdatesMessage<P::Components>>>,

    // BOTH SEND/RECEIVE
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
            buffered_updates: Default::default(),
            collect_changes_since_this_tick: BevyTick::new(0),
            latest_tick: Tick(0),
        }
    }
}

impl<P: Protocol> GroupChannel<P> {
    pub(crate) fn update_collect_changes_since_this_tick(&mut self, bevy_tick: BevyTick) {
        // the bevy_tick passed is either at receive or send, and is always more recent
        // than the previous bevy_tick

        // if bevy_tick is bigger than current tick, set current_tick to bevy_tick
        // if bevy_tick.is_newer_than(self.collect_changes_since_this_tick, BevyTick::MAX) {
        self.collect_changes_since_this_tick = bevy_tick;
        // }
    }

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

    fn read_buffered_updates(&mut self) -> Vec<(Tick, EntityUpdatesMessage<P::Components>)> {
        // go through all the buffered updates whose last_action_tick has been reached
        // (the update's last_action_tick <= latest_tick)
        let not_ready = self.buffered_updates.split_off(&(self.latest_tick + 1));

        let mut res = vec![];
        let buffered_updates_to_consider = std::mem::take(&mut self.buffered_updates);
        for (necessary_action_tick, updates) in buffered_updates_to_consider.into_iter() {
            for (tick, update) in updates {
                // only push the update if the update's tick is more recent than the entity's current latest_tick
                if self.latest_tick < tick {
                    self.latest_tick = tick;
                    res.push((tick, update));
                }
            }
        }
        self.buffered_updates = not_ready;
        res
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use crate::tests::protocol::*;

    use super::*;

    // TODO: add tests for replication with entity relations!
    #[test]
    fn test_buffer_replication_messages() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut manager = ReplicationManager::<MyProtocol>::new(receiver);

        let entity_1 = Entity::from_raw(0);
        let entity_2 = Entity::from_raw(1);
        let entity_3 = Entity::from_raw(2);
        let group_1 = ReplicationGroupId(0);
        let group_2 = ReplicationGroupId(1);

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
                last_action_tick: Tick(3),
                ..Default::default()
            },
        );

        // updates should be grouped with actions
        manager.prepare_entity_spawn(entity_1, group_1);
        manager.prepare_component_insert(
            entity_1,
            group_1,
            MyComponentsProtocol::Component1(Component1(1.0)),
        );
        manager.prepare_component_remove(entity_1, group_1, MyComponentsProtocolKind::Component2);
        manager.prepare_entity_update(
            entity_1,
            group_1,
            MyComponentsProtocol::Component3(Component3(3.0)),
        );

        // handle another entity in the same group: will be added to EntityActions as well
        manager.prepare_entity_update(
            entity_2,
            group_1,
            MyComponentsProtocol::Component2(Component2(4.0)),
        );

        manager.prepare_entity_update(
            entity_3,
            group_2,
            MyComponentsProtocol::Component3(Component3(5.0)),
        );

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
                        spawn: true,
                        despawn: false,
                        insert: vec![MyComponentsProtocol::Component1(Component1(1.0))],
                        remove: HashSet::from_iter(vec![MyComponentsProtocolKind::Component2]),
                        updates: vec![MyComponentsProtocol::Component3(Component3(3.0))],
                    }
                ),
                (
                    entity_2,
                    EntityActions {
                        spawn: false,
                        despawn: false,
                        insert: vec![],
                        remove: HashSet::default(),
                        updates: vec![MyComponentsProtocol::Component2(Component2(4.0))],
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
                    last_action_tick: Tick(3),
                    updates: vec![(
                        entity_3,
                        vec![MyComponentsProtocol::Component3(Component3(5.0))]
                    )],
                })
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
            Tick(2)
        );
    }

    #[allow(clippy::get_first)]
    #[test]
    fn test_recv_replication_messages() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut manager = ReplicationManager::<MyProtocol>::new(receiver);

        let group_id = ReplicationGroupId(0);
        // recv an actions message that is too old: should be ignored
        manager.recv_message(
            ReplicationMessage {
                group_id,
                data: ReplicationMessageData::Actions(EntityActionMessage {
                    sequence_id: MessageId(0) - 1,
                    actions: Default::default(),
                }),
            },
            Tick(0),
        );
        assert_eq!(
            manager
                .group_channels
                .get(&group_id)
                .unwrap()
                .actions_pending_recv_message_id,
            MessageId(0)
        );
        assert!(manager
            .group_channels
            .get(&group_id)
            .unwrap()
            .actions_recv_message_buffer
            .is_empty());

        // recv an actions message: in order, should be buffered
        manager.recv_message(
            ReplicationMessage {
                group_id: ReplicationGroupId(0),
                data: ReplicationMessageData::Actions(EntityActionMessage {
                    sequence_id: MessageId(0),
                    actions: Default::default(),
                }),
            },
            Tick(0),
        );
        assert!(manager
            .group_channels
            .get(&group_id)
            .unwrap()
            .actions_recv_message_buffer
            .get(&MessageId(0))
            .is_some());

        // add an updates message
        manager.recv_message(
            ReplicationMessage {
                group_id: ReplicationGroupId(0),
                data: ReplicationMessageData::Updates(EntityUpdatesMessage {
                    last_action_tick: Tick(0),
                    updates: Default::default(),
                }),
            },
            Tick(1),
        );
        assert!(manager
            .group_channels
            .get(&group_id)
            .unwrap()
            .buffered_updates
            .get(&Tick(0))
            .unwrap()
            .get(&Tick(1))
            .is_some());

        // add updates before actions (last_action_tick is 2)
        manager.recv_message(
            ReplicationMessage {
                group_id: ReplicationGroupId(0),
                data: ReplicationMessageData::Updates(EntityUpdatesMessage {
                    last_action_tick: Tick(2),
                    updates: Default::default(),
                }),
            },
            Tick(4),
        );
        assert!(manager
            .group_channels
            .get(&group_id)
            .unwrap()
            .buffered_updates
            .get(&Tick(2))
            .unwrap()
            .get(&Tick(4))
            .is_some());

        // read messages: only read the first action and update
        let read_messages = manager.read_messages();
        let replication_data = &read_messages.first().unwrap().1;
        assert_eq!(replication_data.get(0).unwrap().0, Tick(0));
        assert_eq!(replication_data.get(1).unwrap().0, Tick(1));

        // recv actions-3: should be buffered, we are still waiting for actions-2
        manager.recv_message(
            ReplicationMessage {
                group_id: ReplicationGroupId(0),
                data: ReplicationMessageData::Actions(EntityActionMessage {
                    sequence_id: MessageId(2),
                    actions: Default::default(),
                }),
            },
            Tick(3),
        );
        assert!(manager.read_messages().is_empty());

        // recv actions-2: we should now be able to read actions-2, actions-3, updates-4
        manager.recv_message(
            ReplicationMessage {
                group_id: ReplicationGroupId(0),
                data: ReplicationMessageData::Actions(EntityActionMessage {
                    sequence_id: MessageId(1),
                    actions: Default::default(),
                }),
            },
            Tick(2),
        );
        let read_messages = manager.read_messages();
        let replication_data = &read_messages.first().unwrap().1;
        assert_eq!(replication_data.len(), 3);
        assert_eq!(replication_data.get(0).unwrap().0, Tick(2));
        assert_eq!(replication_data.get(1).unwrap().0, Tick(3));
        assert_eq!(replication_data.get(2).unwrap().0, Tick(4));
    }
}
