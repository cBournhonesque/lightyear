use crate::prelude::{Channel, ChannelKind, ClientId, Message, NetworkTarget};
use bevy::prelude::Event;

/// This event is emitted whenever we receive a message from the remote
#[derive(Event, Debug)]
pub struct ReceiveMessage<M: Message> {
    pub message: M,
    // TODO: this is not ideal. Should we have PeerId that is either ClientId or Server?
    /// The client that sent the message.
    /// If the server sent the message, we will just put ClientId::Local(0) here
    pub from: ClientId,
}

impl<M: Message> ReceiveMessage<M> {
    pub fn new(message: M, from: ClientId) -> Self {
        Self { message, from }
    }

    pub fn message(&self) -> &M {
        &self.message
    }

    pub fn from(&self) -> ClientId {
        self.from
    }
}

/// Write to this event to buffer a message to be sent
/// The `ConnectionManager` will read these events and send them through the transport
#[derive(Event, Debug)]
pub struct SendMessage<M: Message> {
    pub message: M,
    pub channel: ChannelKind,
    pub to: NetworkTarget,
}

impl<M: Message> SendMessage<M> {
    pub fn new<C: Channel>(message: M, to: NetworkTarget) -> Self {
        Self { message, channel: ChannelKind::of::<C>(), to }
    }

    pub fn message(&self) -> &M {
        &self.message
    }

    pub fn to(&self) -> &NetworkTarget {
        &self.to
    }
}