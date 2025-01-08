use crate::prelude::{Channel, ChannelKind, Message};
use crate::shared::replication::network_target::NetworkTarget;
use bytes::Bytes;

/// Shared trait between client and server to send messages to a target
pub trait MessageSend: private::InternalMessageSend {
    /// Send a message to a target via a channel
    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: M,
        target: NetworkTarget,
    ) {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }

    /// Send bytes to a target via a channel
    fn send_bytes_to_target<C: Channel>(&mut self, bytes: Bytes, target: NetworkTarget) {
        self.erased_send_bytes_to_target(bytes, ChannelKind::of::<C>(), target)
    }
}

pub(crate) mod private {
    use super::*;
    pub trait InternalMessageSend {
        fn erased_send_message_to_target<M: Message>(
            &mut self,
            message: M,
            channel_kind: ChannelKind,
            target: NetworkTarget,
        );

        fn erased_send_bytes_to_target(
            &mut self,
            bytes: Bytes,
            channel_kind: ChannelKind,
            target: NetworkTarget,
        );
    }
}
