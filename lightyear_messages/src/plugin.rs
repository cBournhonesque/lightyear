
// Current design:
// - you call `Events<ToServer<SendMessage<M>>>` to send a message
// - we have a type-erased system that reads from all these events, serializes and stores them
//   in the MessageManager

// Each Link/Transport entity.
// - You can call `ChannelSender::<C>::send_message::<M>()`
// - We


// Should the EntityMap be stored on the Link? or on the Transport?
// Or a different component?

// Borrowing API:
// - You call ChannelSender::<C>.send_message::<M>(
//     &MessageRegistry,
//     &mut EntityMapper,  -> taken from the entity?
//     &mut Transport,  -> taken from the entity?
//   )
//
// And it will:
// - call the message registry to serialize your bytes
// - call the entity mapper to map your entities
// - buffer the bytes on ChannelSenderEnum contained in the Sender

// Non-Borrowing API (Events):
// - You create a message `SendMessage<M>::new::<C>(message, entity)`
// - You call `Events<SendMessage<M>>` to send a message
// - One system reads from all these events and using the entity + channel_kind
//   calls the correct type-erased `ChannelSender::<C>::send_message::<M>()`
//   with the correct EntityMapper and Serializer
//
// Instead of `entity` as second-argument, you could provide `NetworkTarget`,
// and we will find the correct entities that correspond to this target.
// We could have a trait `ToTransportEntity` implemented for Entity, Vec<Entity>, NetworkTarget, etc.

use crate::registry::{MessageError, MessageRegistry};
use crate::Message;
use lightyear_packet::channel::builder::ChannelSender;
use lightyear_packet::channel::Channel;
use lightyear_packet::prelude::Transport;

// TODO: provide an api where we send to the link directly?

// Extension trait so that we can implement it for ChannelSender<C>
trait SendMessage<M: Message> {
    fn send_message<M>(
        &mut self,
        message: M,
        priority: f32,
        registry: &MessageRegistry,
        transport: &mut Transport,
        // TODO: separate error type for SendMessage and ReceiveMessage
    ) -> Result<(), MessageError>;
}

impl<C: Channel, M: Message> SendMessage<M> for ChannelSender<C> {

    fn send_message<M>(&mut self, message: &M, priority: f32, registry: &MessageRegistry, transport: &mut Transport) -> Result<(), MessageError> {
        registry.serialize(message, &mut self.writer, &mut transport.send_mapper)?;
        let message_id = self.sender.buffer_send(self.writer.split(), priority)?;
        Ok(())
    }
}