use crate::packet::message_manager::MessageManager;
use crate::replication::ReplicationMessage;
use crate::Entity;
use crate::{Channel, ChannelRegistry, MessageContainer, Protocol};

pub struct ReplicationManager<P: Protocol> {
    pub message_manager: MessageManager<ReplicationMessage<P::Components, P::ComponentKinds>>,
}

impl<P: Protocol> ReplicationManager<P> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            message_manager: MessageManager::new(channel_registry),
        }
    }

    pub fn buffer_spawn_entity<C: Channel>(&mut self, entity: Entity) {
        let message = MessageContainer::new(ReplicationMessage::SpawnEntity(entity));
        self.message_manager.buffer_send::<C>(message);
    }
}
