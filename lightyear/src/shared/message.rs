use crate::prelude::{Channel, ChannelKind, Message, NetworkTarget};
use crate::protocol::EventContext;
use bevy::prelude::Resource;
use std::fmt::Debug;
use std::hash::Hash;

pub(crate) trait MessageSend: Resource {
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
