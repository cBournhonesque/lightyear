//! General struct handling replication
use std::iter::Extend;

use anyhow::Context;
use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::Entity;
use bevy::utils::petgraph::data::ElementIterator;
use bevy::utils::{EntityHashMap, HashMap, HashSet};
use crossbeam_channel::Receiver;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::filter::FilterExt;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::_reexport::{EntityActionsChannel, EntityUpdatesChannel, FromType};
use crate::packet::message::MessageId;
use crate::prelude::{MapEntities, ShouldBePredicted, Tick};
use crate::protocol::channel::ChannelKind;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::component::{ComponentBehaviour, ComponentKindBehaviour};
use crate::protocol::Protocol;
use crate::shared::replication::components::{Replicate, ReplicationGroupId};

use super::{EntityActionMessage, EntityActions, EntityUpdatesMessage, ReplicationMessageData};

pub(crate) struct ReplicationSender<P: Protocol> {
    // SEND
    // TODO: this is unused by server-send, should we just move it to client-connection?
    //  in general, we should have some parts of replication-sender/receiver that are shared across all connections!
    /// Stores the last `Replicate` component for each replicated entity owned by the current world (the world that sends replication updates)
    /// Needed to know the value of the Replicate component after the entity gets despawned, to know how we replicate the EntityDespawn
    pub replicate_component_cache: EntityHashMap<Entity, Replicate<P>>,
    /// Get notified whenever a message-id that was sent has been received by the remote
    pub updates_ack_tracker: Receiver<MessageId>,
    /// Map from message-id to the corresponding group-id that sent this update message, as well as the bevy ChangeTick
    /// when we sent the message. (so that when it's acked, we know we only need to include updates that happened after that tick,
    /// for that replication group)
    pub updates_message_id_to_group_id: HashMap<MessageId, (ReplicationGroupId, BevyTick)>,
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

    // BOTH
    /// Buffer to so that we have an ordered receiver per group
    pub group_channels: EntityHashMap<ReplicationGroupId, GroupChannel>,
}

impl<P: Protocol> ReplicationSender<P> {
    pub(crate) fn new(updates_ack_tracker: Receiver<MessageId>) -> Self {
        Self {
            // SEND
            replicate_component_cache: EntityHashMap::default(),
            updates_ack_tracker,
            updates_message_id_to_group_id: Default::default(),
            pending_actions: EntityHashMap::default(),
            pending_updates: EntityHashMap::default(),
            pending_unique_components: EntityHashMap::default(),
            // BOTH
            group_channels: Default::default(),
        }
    }

    // TODO: call this in a system after receive
    /// We call this after receive stage; to update the bevy_tick at which we received entity updates for each group
    pub(crate) fn recv_update_acks(&mut self) {
        // TODO: handle errors that are not channel::isEmpty
        while let Ok(message_id) = self.updates_ack_tracker.try_recv() {
            if let Some((group_id, bevy_tick)) =
                self.updates_message_id_to_group_id.get(&message_id)
            {
                let channel = self.group_channels.entry(*group_id).or_default();
                channel.update_collect_changes_since_this_tick(*bevy_tick);
            } else {
                error!(
                    "Received an update message-id ack but we don't have the corresponding group"
                );
            }
        }
    }
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
///
/// - all component inserts/removes/updates for an entity to be grouped together in a single message
impl<P: Protocol> ReplicationSender<P> {
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

        // special case for ShouldBePredicted:
        // if we have already have a ShouldBePredicted component inserted from `prediction_target`
        // we overwrite it if we are inserting a ShouldBePredicted component for pre-prediction
        let mut force_insert = false;
        if kind == <P::ComponentKinds as FromType<ShouldBePredicted>>::from_type()
            && component
                .clone()
                .try_into()
                .is_ok_and(|s| s.client_entity.is_some())
        {
            trace!("force inserting ShouldBePredicted component for pre-predicted entity");
            // removed the existing ShouldBePredicted
            self.pending_actions
                .entry(group)
                .or_default()
                .entry(entity)
                .or_default()
                .insert
                .retain(|c| P::ComponentKinds::from(c) != kind);
            force_insert = true;
        }

        if self
            .pending_unique_components
            .entry(group)
            .or_default()
            .entry(entity)
            .or_default()
            .contains(&kind)
            && !force_insert
        {
            if kind == <P::ComponentKinds as FromType<ShouldBePredicted>>::from_type()
                && component
                    .clone()
                    .try_into()
                    .is_ok_and(|s| s.client_entity.is_none())
            {
                // do not emit a warning if we are trying to insert a ShouldBePredicted component
                // because of prediction_target, but there already is a ShouldBePredicted component because of
                // pre-prediction
                return;
            }
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
            debug!(
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
            channel.last_action_tick = Some(tick);
            messages.push((
                ChannelKind::of::<EntityActionsChannel>(),
                group_id,
                ReplicationMessageData::Actions(EntityActionMessage {
                    sequence_id: message_id,
                    // TODO: maybe we can just send the HashMap directly?
                    actions: Vec::from_iter(actions.into_iter()),
                }),
            ));
            debug!("final action messages to send: {:?}", messages);
        }
        // send the remaining updates
        for (group_id, updates) in self.pending_updates.drain() {
            let channel = self.group_channels.entry(group_id).or_default();
            messages.push((
                ChannelKind::of::<EntityUpdatesChannel>(),
                group_id,
                ReplicationMessageData::Updates(EntityUpdatesMessage {
                    // SAFETY: the last action tick is always set because we send Actions before Updates
                    last_action_tick: channel.last_action_tick,
                    // TODO: maybe we can just send the HashMap directly?
                    updates: Vec::from_iter(updates.into_iter()),
                }),
            ));
        }

        if !messages.is_empty() {
            debug!(?messages, "Sending replication messages");
        }

        // clear send buffers
        self.pending_unique_components.clear();
        messages
    }
}

/// Channel to keep track of sending replication messages for a given Group
#[derive(Debug)]
pub struct GroupChannel {
    pub actions_next_send_message_id: MessageId,
    // TODO: maybe also keep track of which Tick this bevy-tick corresponds to? (will enable doing diff-compression)
    // bevy tick when we received an ack of an update for this group
    // at the start it's None, and we collect any changes
    pub collect_changes_since_this_tick: Option<BevyTick>,
    // last tick for which we sent an action message
    pub last_action_tick: Option<Tick>,
}

impl Default for GroupChannel {
    fn default() -> Self {
        Self {
            actions_next_send_message_id: MessageId(0),
            last_action_tick: None,
            collect_changes_since_this_tick: None,
        }
    }
}

impl GroupChannel {
    pub(crate) fn update_collect_changes_since_this_tick(&mut self, bevy_tick: BevyTick) {
        // the bevy_tick passed is either at receive or send, and is always more recent
        // than the previous bevy_tick

        trace!(?bevy_tick, "Update acked update tick");
        // if bevy_tick is bigger than current tick, set current_tick to bevy_tick
        // if bevy_tick.is_newer_than(self.collect_changes_since_this_tick, BevyTick::MAX) {
        self.collect_changes_since_this_tick = Some(bevy_tick);
        // }
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
        let mut manager = ReplicationSender::<MyProtocol>::new(receiver);

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
                last_action_tick: Some(Tick(3)),
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
                    last_action_tick: Some(Tick(3)),
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
            Some(Tick(2))
        );
    }
}
