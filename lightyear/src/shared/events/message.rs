use crate::prelude::{Channel, ChannelKind, ClientId, Message, NetworkTarget};
use bevy::prelude::Event;
use core::marker::PhantomData;

// Note: we cannot simply use `ReceiveMessage<M>` because we would have no way of differentiating
// between the client or the server receiving a message in host-server mode
/// This event is emitted whenever we receive a message from the remote
#[derive(Event, Debug)]
pub struct ReceiveMessage<M: Message, Marker: 'static> {
    pub message: M,
    // TODO: this is not ideal. Should we have PeerId that is either ClientId or Server?
    /// The client that sent the message.
    /// If the server sent the message, we will just put ClientId::Local(0) here
    pub from: ClientId,
    marker: PhantomData<Marker>,
}

impl<M: Message, Marker: 'static> ReceiveMessage<M, Marker> {
    pub fn new(message: M, from: ClientId) -> Self {
        Self {
            message,
            from,
            marker: PhantomData,
        }
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
pub struct SendMessage<M: Message, Marker: 'static> {
    pub message: M,
    pub channel: ChannelKind,
    pub to: NetworkTarget,
    marker: PhantomData<Marker>,
}

impl<M: Message, Marker: 'static> SendMessage<M, Marker> {
    pub fn new<C: Channel>(message: M) -> Self {
        Self::new_with_target::<C>(message, NetworkTarget::None)
    }

    pub fn new_with_target<C: Channel>(message: M, to: NetworkTarget) -> Self {
        Self {
            message,
            channel: ChannelKind::of::<C>(),
            to,
            marker: PhantomData,
        }
    }

    pub fn message(&self) -> &M {
        &self.message
    }

    pub fn to(&self) -> &NetworkTarget {
        &self.to
    }
}
