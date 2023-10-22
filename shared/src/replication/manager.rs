use std::collections::HashMap;

use bevy_ecs::prelude::{Entity, World};
use tracing::debug;

use crate::connection::ProtocolMessage;
use crate::replication::entity_map::EntityMap;
use crate::replication::{Replicate, ReplicationMessage};
use crate::{ChannelKind, ComponentBehaviour, Protocol};

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
}

impl<P: Protocol> Default for ReplicationManager<P> {
    fn default() -> Self {
        Self {
            entity_map: EntityMap::default(),
            remote_entity_status: HashMap::new(),
            individual_component_updates: HashMap::new(),
        }
    }
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
impl<P: Protocol> ReplicationManager<P> {
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

    pub(crate) fn send_entity_update(&mut self, entity: Entity, replicate: Replicate) -> bool {
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
    pub(crate) fn apply_world(&mut self, world: &mut World, message: ProtocolMessage<P>) {
        // TODO: add span
        match message {
            ProtocolMessage::Replication(replication) => match replication {
                ReplicationMessage::SpawnEntity(entity, components) => {
                    // let local_entity = world.spawn(components.into()).id();

                    // TODO: we only run spawn_entity if we don't already have an entity in the process of being spawned
                    //  so we need a data-structure to keep track of entities that are being spawned
                    //  or do we? I'm not sure we would send this twice, because of the bevy system logic
                    //  but maybe we would do, if we remove Replicate and then Re-add it?

                    // TODO: optimize by using batch functions
                    let mut local_entity_mut = world.spawn_empty();
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
                        world.despawn(entity);
                    }
                }
                ReplicationMessage::InsertComponent(entity, component) => {
                    // TODO:

                    // TODO: add kind in debug message?
                    if let Some(local_entity) = self.entity_map.get_local(entity) {
                        if let Some(mut entity_mut) = world.get_entity_mut(*local_entity) {
                            // TODO: convert the component into inner
                            //  maybe for each
                            component.insert(&mut entity_mut);
                            // entity_mut.insert(c);
                        } else {
                            debug!("Could not insert component because local entity {:?} was not found", local_entity);
                        }
                    }
                    debug!(
                        "Could not insert component because remote entity {:?} was not found",
                        entity
                    );
                }
                ReplicationMessage::RemoveComponent(entity, component_kind) => {
                    if let Some(local_entity) = self.entity_map.get_local(entity) {
                        if let Some(mut entity_mut) = world.get_entity_mut(*local_entity) {
                            // TODO: HOW TO GET COMPONENT TYPE FROM KIND? (i.e. enum inner type from kind)
                            // entity_mut.remove::<component_kind>();
                        } else {
                            debug!("Could not remove component because local entity {:?} was not found", local_entity);
                        }
                    }
                    debug!(
                        "Could not remove component because remote entity {:?} was not found",
                        entity
                    );
                }
                ReplicationMessage::EntityUpdate(entity, components) => {
                    let mut local_entity_mut =
                        self.entity_map.get_by_remote_or_spawn(world, entity);
                    for component in components {
                        component.insert(&mut local_entity_mut);
                    }
                }
            },
            ProtocolMessage::Message(_) => {}
        }
    }

    // pub fn buffer_spawn_entity<C: Channel>(&mut self, entity: Entity) {
    //     let message = MessageContainer::new(ReplicationMessage::SpawnEntity(entity));
    //     self.message_manager.buffer_send::<C>(message);
    // }
}
