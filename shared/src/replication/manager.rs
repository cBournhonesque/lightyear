use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::{FromWorld, World};
use tracing::debug;

use crate::connection::ProtocolMessage;
use crate::replication::entity_map::EntityMap;
use crate::replication::{Replicate, ReplicationMessage};
use crate::{ComponentBehaviour, Protocol};

#[derive(Default)]
pub struct ReplicationManager {
    pub entity_map: EntityMap,
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
impl ReplicationManager {
    /// Apply any replication messages to the world
    pub(crate) fn apply_world<P: Protocol>(
        &mut self,
        world: &mut World,
        message: ProtocolMessage<P>,
    ) {
        // TODO: add span
        match message {
            ProtocolMessage::Replication(replication) => match replication {
                ReplicationMessage::SpawnEntity(entity, components) => {
                    // let local_entity = world.spawn(components.into()).id();

                    // TODO: optimize by using batch functions
                    let mut local_entity_mut = world.spawn_empty();
                    for component in components {
                        component.insert(&mut local_entity_mut);
                    }
                    self.entity_map.insert(entity, local_entity_mut.id());
                }
                ReplicationMessage::DespawnEntity(entity) => {
                    if let Some(local_entity) = self.entity_map.remove_by_remote(entity) {
                        world.despawn(entity);
                    }
                }
                ReplicationMessage::InsertComponent(entity, component) => {
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
