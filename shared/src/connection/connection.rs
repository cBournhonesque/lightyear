use crate::packet::message_manager::MessageManager;
use crate::replication::manager::ReplicationManager;
use crate::{ChannelRegistry, Protocol};

/// Wrapper to: send/receive messages via channels to a remote address
/// By splitting the data into packets and sending them through a given transport
pub struct Connection<P: Protocol> {
    pub message_manager: MessageManager<P::Message>,
    pub replication_manager: ReplicationManager<P>,
}

impl<P: Protocol> Connection<P> {
    pub fn new(channel_registry: &ChannelRegistry) -> Self {
        Self {
            message_manager: MessageManager::new(channel_registry),
            replication_manager: ReplicationManager::new(channel_registry),
        }
    }
}
