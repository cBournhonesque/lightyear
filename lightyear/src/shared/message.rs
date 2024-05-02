use crate::prelude::{Channel, ChannelKind, Message, NetworkTarget};
use crate::protocol::EventContext;
use bevy::prelude::Resource;
use std::fmt::Debug;
use std::hash::Hash;

pub(crate) trait MessageSend: Resource {
    /// Type of the context associated with the events emitted by this replication plugin
    type EventContext: EventContext;
    /// Marker to identify the type of the ReplicationSet component
    /// This is mostly relevant in the unified mode, where a ReplicationSet can be added several times
    /// (in the client and the server replication plugins)
    type SetMarker: Debug + Hash + Send + Sync + Eq + Clone;
    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> anyhow::Result<()>;

    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) -> anyhow::Result<()>;
}
