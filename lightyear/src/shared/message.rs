use crate::prelude::{Channel, ChannelKind, Message};
use crate::shared::replication::network_target::NetworkTarget;
use bevy::prelude::Resource;
use core::error::Error;

/// Shared trait between client and server to send messages to a target
pub trait MessageSend: private::InternalMessageSend {
    /// Send a message to a target via a channel
    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) -> Result<(), Self::Error> {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }
}

pub(crate) mod private {
    use super::*;
    pub trait InternalMessageSend: Resource {
        type Error: Error;
        fn erased_send_message_to_target<M: Message>(
            &mut self,
            message: &M,
            channel_kind: ChannelKind,
            target: NetworkTarget,
        ) -> Result<(), Self::Error>;
    }
}
