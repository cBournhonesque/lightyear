//! General struct handling replication
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use bevy::prelude::{Entity, World};
use tracing::{debug, error, info, trace, trace_span};
use tracing_subscriber::filter::FilterExt;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use super::entity_map::EntityMap;
use super::{Replicate, ReplicationMessage};
use crate::connection::message::ProtocolMessage;
use crate::protocol::channel::ChannelKind;
use crate::protocol::component::{ComponentBehaviour, ComponentKindBehaviour};
use crate::protocol::Protocol;

// TODO: maybe store additional information about the entity?
//  (e.g. the value of the replicate component)?
pub enum EntityStatus {
    JustSpawned,
    Spawning,
    Spawned,
}

pub(crate) struct ReplicationManager<P: Protocol> {
    pub entity_map: EntityMap,
    pub remote_entity_status: HashMap<Entity, EntityStatus>,

    /// messages that are being written. We need to hold a buffer of messages because components actions/updates
    /// are being buffered individually but we want to group them inside a message
    pub pending_spawns: HashMap<Entity, Vec<P::Components>>,
    pub pending_despawns: HashSet<Entity>,
    pub pending_inserts: HashMap<Entity, Vec<P::Components>>,
    pub pending_removes: HashMap<Entity, Vec<P::ComponentKinds>>,
    pub pending_updates: HashMap<Entity, Vec<P::Components>>,
    pub entity_actions_channel: HashMap<Entity, ChannelKind>,
    pub entity_updates_channel: HashMap<Entity, ChannelKind>,
    // pub global_replication_data: &'a ReplicationData,
}

impl<P: Protocol> Default for ReplicationManager<P> {
    fn default() -> Self {
        Self {
            entity_map: EntityMap::default(),
            remote_entity_status: HashMap::new(),
            pending_spawns: HashMap::new(),
            pending_despawns: HashSet::default(),
            pending_inserts: HashMap::default(),
            pending_removes: HashMap::default(),
            pending_updates: HashMap::default(),
            entity_actions_channel: HashMap::default(),
            entity_updates_channel: HashMap::default(),
            // global_replication_data,
        }
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
    pub(crate) fn prepare_entity_spawn(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        channel_kind: ChannelKind,
    ) {
        // TODO: send error if there was another channel for this entity?
        self.entity_actions_channel.insert(entity, channel_kind);

        // TODO: check if we have already sent SpawnMessage for this entity?

        self.pending_spawns
            .entry(entity)
            .and_modify(|_| {
                error!("Entity already has a pending spawn");
            })
            .or_insert(components);

        // // if we have already sent the Spawn Entity, don't do it again
        // if self.remote_entity_status.get(&entity).is_some() {
        //     return false;
        // }
        // self.remote_entity_status
        //     .insert(entity, EntityStatus::Spawning);
        // true
    }

    pub(crate) fn prepare_entity_despawn(&mut self, entity: Entity, channel_kind: ChannelKind) {
        self.entity_actions_channel.insert(entity, channel_kind);
        // TODO: check if we have already sent DespawnMessage for this entity? (send error message?)
        //  or if the SpawnEntity was sent/received?
        self.pending_despawns.insert(entity);
    }

    // we want to send all component inserts that happen together for the same entity in a single message
    // (because otherwise the inserts might be received at different packets/ticks by the remote, and
    // the remote might expect the components insert to be received at the same time)
    pub(crate) fn prepare_component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        channel: ChannelKind,
    ) {
        self.entity_actions_channel.insert(entity, channel);

        // if the entity is about to be despawned, don't send the insert
        if self.pending_despawns.contains(&entity) {
            return;
        }

        // if the entity is spawning, add the component insert to the spawn message directly
        // NOTE: this works because we handle spawns before component inserts
        if let Some(components) = self.pending_spawns.get_mut(&entity) {
            components.push(component);
        } else {
            self.pending_inserts
                .entry(entity)
                .or_default()
                .push(component);
        }
    }

    pub(crate) fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component: P::ComponentKinds,
        channel: ChannelKind,
    ) {
        // if the entity is about to be despawned, don't send the remove
        if self.pending_despawns.contains(&entity) {
            return;
        }

        self.entity_actions_channel.insert(entity, channel);
        self.pending_removes
            .entry(entity)
            .or_default()
            .push(component);
    }

    pub(crate) fn prepare_entity_update(
        &mut self,
        entity: Entity,
        component: P::Components,
        channel: ChannelKind,
    ) {
        self.entity_updates_channel.insert(entity, channel);

        // if the entity is about to be despawned, don't send the update
        if self.pending_despawns.contains(&entity) {
            return;
        }

        // TODO: if the component is spawning, should we put the update in the spawn message?
        //  because else the update might arrive before the entity spawn
        self.pending_updates
            .entry(entity)
            .or_default()
            .push(component);
    }

    /// Finalize the replication messages
    pub(crate) fn finalize(&mut self) -> Vec<(ChannelKind, ProtocolMessage<P>)> {
        let mut messages = Vec::new();

        // entity actions
        for (entity, components) in self.pending_spawns.drain() {
            // SAFETY: we made sure that each entity has a channel
            let channel = self.entity_actions_channel.get(&entity).unwrap();
            messages.push((
                *channel,
                ProtocolMessage::Replication(ReplicationMessage::SpawnEntity(entity, components)),
            ));
        }
        for entity in self.pending_despawns.drain() {
            // SAFETY: we made sure that each entity has a channel
            let channel = self.entity_actions_channel.get(&entity).unwrap();
            messages.push((
                *channel,
                ProtocolMessage::Replication(ReplicationMessage::DespawnEntity(entity)),
            ));
        }
        for (entity, components) in self.pending_inserts.drain() {
            // SAFETY: we made sure that each entity has a channel
            let channel = self.entity_actions_channel.get(&entity).unwrap();
            messages.push((
                *channel,
                ProtocolMessage::Replication(ReplicationMessage::InsertComponent(
                    entity, components,
                )),
            ));
        }
        for (entity, components) in self.pending_removes.drain() {
            // SAFETY: we made sure that each entity has a channel
            let channel = self.entity_actions_channel.get(&entity).unwrap();
            messages.push((
                *channel,
                ProtocolMessage::Replication(ReplicationMessage::RemoveComponent(
                    entity, components,
                )),
            ));
        }

        // entity updates
        for (entity, components) in self.pending_updates.drain() {
            // SAFETY: we made sure that each entity has a channel
            let channel = self.entity_updates_channel.remove(&entity).unwrap();
            messages.push((
                channel,
                ProtocolMessage::Replication(ReplicationMessage::EntityUpdate(entity, components)),
            ));
        }

        // clear
        self.entity_actions_channel.clear();

        messages
    }

    /// Apply any replication messages to the world
    pub(crate) fn apply_world(
        &mut self,
        world: &mut World,
        replication: ReplicationMessage<P::Components, P::ComponentKinds>,
    ) {
        let _span = trace_span!("Apply received replication message to world").entered();
        match replication {
            ReplicationMessage::SpawnEntity(entity, components) => {
                let component_kinds = components
                    .iter()
                    .map(|c| c.into())
                    .collect::<Vec<P::ComponentKinds>>();
                debug!(remote_entity = ?entity, ?component_kinds, "Received spawn entity");

                // TODO: we only run spawn_entity if we don't already have an entity in the process of being spawned
                //  so we need a data-structure to keep track of entities that are being spawned
                //  or do we? I'm not sure we would send this twice, because of the bevy system logic
                //  but maybe we would do, if we remove Replicate and then Re-add it?

                // Ignore if we already received the entity
                if self.entity_map.get_local(entity).is_some() {
                    return;
                }
                let mut local_entity_mut = world.spawn_empty();

                // TODO: optimize by using batch functions?
                for component in components {
                    component.insert(&mut local_entity_mut);
                }
                self.entity_map.insert(entity, local_entity_mut.id());
            }
            ReplicationMessage::DespawnEntity(entity) => {
                // TODO: we only run this if the entity has been confirmed to be spawned on client?
                //  or should we send the message right away and let the receiver handle the ordering?
                //  (what if they receive despawn before spawn?)
                if let Some(local_entity) = self.entity_map.remove_by_remote(entity) {
                    world.despawn(local_entity);
                }
            }
            ReplicationMessage::InsertComponent(entity, components) => {
                let kinds = components
                    .iter()
                    .map(|c| c.into())
                    .collect::<Vec<P::ComponentKinds>>();
                debug!(remote_entity = ?entity, ?kinds, "Received InsertComponent");
                // it's possible that we received InsertComponent before the entity actually exists.
                // In that case, we need to spawn the entity first.
                // TODO: this might not be what we want? imagine we receive a DespawnEntity or RemoveComponent right before that?
                let mut local_entity_mut = self.entity_map.get_by_remote_or_spawn(world, entity);
                // TODO: maybe check if the component already exists?
                for component in components {
                    component.insert(&mut local_entity_mut);
                }
            }
            ReplicationMessage::RemoveComponent(entity, component_kinds) => {
                debug!(remote_entity = ?entity, kinds = ?component_kinds, "Received RemoveComponent");
                if let Some(local_entity) = self.entity_map.get_local(entity) {
                    if let Some(mut entity_mut) = world.get_entity_mut(*local_entity) {
                        for kind in component_kinds {
                            kind.remove(&mut entity_mut);
                        }
                    } else {
                        debug!(
                            "Could not remove component because local entity {:?} was not found",
                            local_entity
                        );
                    }
                }
                debug!(
                    "Could not remove component because remote entity {:?} was not found",
                    entity
                );
            }
            ReplicationMessage::EntityUpdate(entity, components) => {
                let kinds = components
                    .iter()
                    .map(|c| c.into())
                    .collect::<Vec<P::ComponentKinds>>();
                trace!(?entity, ?kinds, "Received entity update");
                // if the entity does not exist, create it
                let mut local_entity_mut = self.entity_map.get_by_remote_or_spawn(world, entity);
                // TODO: keep track of the components inserted?
                for component in components {
                    component.update(&mut local_entity_mut);
                }
            }
        }
    }

    // pub fn buffer_spawn_entity<C: Channel>(&mut self, entity: Entity) {
    //     let message = MessageContainer::new(ReplicationMessage::SpawnEntity(entity));
    //     self.message_manager.buffer_send::<C>(message);
    // }
}
