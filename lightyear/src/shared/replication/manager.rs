//! General struct handling replication
use std::collections::HashMap;

use bevy::prelude::{Entity, World};
use tracing::{debug, trace_span};

use super::entity_map::EntityMap;
use super::{Replicate, ReplicationMessage};
use crate::connection::message::ProtocolMessage;
use crate::protocol::channel::ChannelKind;
use crate::protocol::component::{ComponentBehaviour, ComponentKindBehaviour};
use crate::protocol::Protocol;

// TODO: maybe store additional information about the entity?
//  (e.g. the value of the replicate component)?
pub enum EntityStatus {
    Spawning,
    Spawned,
}

pub struct ReplicationManager<P: Protocol> {
    pub entity_map: EntityMap,
    pub remote_entity_status: HashMap<Entity, EntityStatus>,
    pub individual_component_updates: HashMap<(Entity, ChannelKind), Vec<P::Components>>,
    // pub global_replication_data: &'a ReplicationData,
}

impl<P: Protocol> Default for ReplicationManager<P> {
    fn default() -> Self {
        Self {
            entity_map: EntityMap::default(),
            remote_entity_status: HashMap::new(),
            individual_component_updates: HashMap::new(),
            // global_replication_data,
        }
    }
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
impl<P: Protocol> ReplicationManager<P> {
    // pub fn new(global_replication_data: &ReplicationData) -> Self {
    //     Self {
    //         entity_map: EntityMap::default(),
    //         remote_entity_status: HashMap::new(),
    //         individual_component_updates: HashMap::new(),
    //
    //         global_replication_data,
    //     }
    // }

    /// Host has spawned an entity, and we want to replicate this to remote
    /// Returns true if we should send a message
    pub(crate) fn send_entity_spawn(&mut self, entity: Entity) -> bool {
        // if we have already sent the Spawn Entity, don't do it again
        if self.remote_entity_status.get(&entity).is_some() {
            return false;
        }
        self.remote_entity_status
            .insert(entity, EntityStatus::Spawning);
        true
    }

    pub(crate) fn send_component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        channel: ChannelKind,
    ) {
        // buffer the component update for that entity
        self.individual_component_updates
            .entry((entity, channel))
            .or_default()
            .push(component);
    }

    pub(crate) fn send_component_remove(
        &mut self,
        _entity: Entity,
        _component: P::ComponentKinds,
        _channel: ChannelKind,
    ) {
        todo!()
    }

    pub(crate) fn send_entity_update_single_component(
        &mut self,
        entity: Entity,
        component: P::Components,
        channel: ChannelKind,
    ) {
        // buffer the component update for that entity
        self.individual_component_updates
            .entry((entity, channel))
            .or_default()
            .push(component);
    }

    pub(crate) fn prepare_send(&mut self) -> Vec<(ChannelKind, ProtocolMessage<P>)> {
        let mut messages = Vec::new();
        for ((entity, channel), components) in self.individual_component_updates.drain() {
            messages.push((
                channel,
                ProtocolMessage::Replication(ReplicationMessage::EntityUpdate(entity, components)),
            ));
        }
        messages
    }

    pub(crate) fn send_entity_update(&mut self, entity: Entity, _replicate: Replicate) -> bool {
        // if we have already sent the Spawn Entity, don't do it again
        if self.remote_entity_status.get(&entity).is_some() {
            return false;
        }
        self.remote_entity_status
            .insert(entity, EntityStatus::Spawning);
        true
    }

    // /// Host has despawned an entity, and we want to replicate this to remote
    // /// Returns true if we should send a message
    // pub(crate) fn send_entity_despawn(&mut self, entity: Entity) -> bool {
    //     // if we have already sent the Spawn Entity, don't do it again
    //     if self.remote_entity_status.get(&entity).is_none() {
    //         panic!("Cannot find spawned entity in host metadata!");
    //         return false;
    //     }
    //     match self.remote_entity_status.get(&entity) {
    //         Some(EntityStatus::Spawning(replicate)) | Some(EntityStatus::Spawned(replicate)) => {
    //             return true;
    //             panic!("Cannot despawn entity that is still spawning!");
    //             return false;
    //         }
    //     }
    //     if let Some(status) = self.remote_entity_status.get(&entity).unwrap() {}
    //
    //     self.remote_entity_status
    //         .insert(entity, EntityStatus::Spawning(replicate));
    //     true
    // }

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
                debug!(?entity, ?component_kinds, "Received spawn entity");
                // let local_entity = world.spawn(components.into()).id();

                // TODO: we only run spawn_entity if we don't already have an entity in the process of being spawned
                //  so we need a data-structure to keep track of entities that are being spawned
                //  or do we? I'm not sure we would send this twice, because of the bevy system logic
                //  but maybe we would do, if we remove Replicate and then Re-add it?

                // Ignore if we already received the entity
                if self.entity_map.get_local(entity).is_some() {
                    return;
                }
                let mut local_entity_mut = world.spawn_empty();

                // TODO: optimize by using batch functions
                for component in components {
                    component.insert(&mut local_entity_mut);
                }
                self.entity_map.insert(entity, local_entity_mut.id());
            }
            ReplicationMessage::DespawnEntity(entity) => {
                // TODO: we only run this if the entity has been confirmed to be spawned on client?
                //  or should we send the message right away and let the receiver handle the ordering?
                //  (what if they receive despawn before spawn?)

                if let Some(_local_entity) = self.entity_map.remove_by_remote(entity) {
                    world.despawn(entity);
                }
            }
            ReplicationMessage::InsertComponent(entity, component) => {
                let kind: P::ComponentKinds = (&component).into();
                debug!(?entity, ?kind, "Received InsertComponent");
                // it's possible that we received InsertComponent before the entity actually exists.
                // In that case, we need to spawn the entity first.
                let mut local_entity_mut = self.entity_map.get_by_remote_or_spawn(world, entity);
                // TODO: maybe check if the component already exists?
                component.insert(&mut local_entity_mut);
            }
            ReplicationMessage::RemoveComponent(entity, component_kind) => {
                debug!(?entity, ?component_kind, "Received RemoveComponent");
                if let Some(local_entity) = self.entity_map.get_local(entity) {
                    if let Some(mut entity_mut) = world.get_entity_mut(*local_entity) {
                        component_kind.remove(&mut entity_mut);
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
                debug!(?entity, ?kinds, "Received entity update");
                // if the entity does not exist, create it
                let mut local_entity_mut = self.entity_map.get_by_remote_or_spawn(world, entity);
                // TODO: keep track of the components inserted?
                for component in components {
                    component.insert(&mut local_entity_mut);
                }
            }
        }
    }

    // pub fn buffer_spawn_entity<C: Channel>(&mut self, entity: Entity) {
    //     let message = MessageContainer::new(ReplicationMessage::SpawnEntity(entity));
    //     self.message_manager.buffer_send::<C>(message);
    // }
}
