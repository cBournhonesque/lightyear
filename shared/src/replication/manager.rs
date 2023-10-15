use crate::connection::ProtocolMessage;
use crate::packet::message_manager::MessageManager;
use crate::replication::entity_map::EntityMap;
use crate::replication::ReplicationMessage;
use crate::{ChannelRegistry, Protocol};
use bevy_ecs::prelude::World;
use tracing::debug;

pub struct ReplicationManager {
    pub entity_map: EntityMap,
}

impl ReplicationManager {
    pub fn new() -> Self {
        Self {
            entity_map: EntityMap::new(),
        }
    }

    pub(crate) fn apply_world<P: Protocol>(
        &mut self,
        world: &mut World,
        message: ProtocolMessage<P>,
    ) {
        // TODO: add span
        match message {
            ProtocolMessage::Replication(replication) => match replication {
                ReplicationMessage::SpawnEntity(entity, components) => {
                    let local_entity = world.spawn(components.into()).id();
                    self.entity_map.insert(entity, local_entity);
                }
                ReplicationMessage::DespawnEntity(entity) => {
                    if let Some(local_entity) = self.entity_map.remove_by_remote(entity) {
                        world.despawn(entity);
                    }
                }
                ReplicationMessage::InsertComponent(entity, component) => {
                    // TODO: add kind in debug message?
                    if let Some(local_entity) = self.entity_map.from_remote(entity) {
                        if let Some(mut entity_mut) = world.get_entity_mut(*local_entity) {
                            // TODO: convert the component into inner
                            entity_mut.insert(component);
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
                    if let Some(local_entity) = self.entity_map.from_remote(entity) {
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
                ReplicationMessage::EntityUpdate(_, _) => {}
            },
            ProtocolMessage::Message(_) => {}
        }
    }

    // pub fn buffer_spawn_entity<C: Channel>(&mut self, entity: Entity) {
    //     let message = MessageContainer::new(ReplicationMessage::SpawnEntity(entity));
    //     self.message_manager.buffer_send::<C>(message);
    // }
}
