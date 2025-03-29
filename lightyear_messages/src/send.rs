use crate::plugin::MessagePlugin;
use crate::registry::serialize::ErasedSerializeFns;
use crate::registry::{MessageError, MessageRegistry};
use crate::{Message, MessageManager};
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::world::FilteredEntityMut;
use bevy::prelude::{Component, Entity, Query, Res};
use lightyear_serde::writer::Writer;
use lightyear_transport::channel::{Channel, ChannelKind};
use lightyear_transport::entity_map::SendEntityMap;
use lightyear_transport::prelude::Transport;
use std::sync::Arc;
use tracing::error;

pub type Priority = f32;

#[derive(Component)]
#[require(MessageManager)]
pub struct MessageSender<M> {
    send: Vec<(M, ChannelKind, Priority)>,
    writer: Writer,
}

impl<M: Message> Default for MessageSender<M> {
    fn default() -> Self {
        Self {
            send: Vec::new(),
            writer: Writer::default(),
        }
    }
}

// SAFETY: the sender must correspond to the correct `MessageSender<M>` type
pub(crate) type SendMessageFn = unsafe fn(
    sender: MutUntyped,
    transport: &Transport,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut SendEntityMap,
) -> Result<(), MessageError>;


impl<M: Message> MessageSender<M> {

    /// Buffers a message to be sent over the channel
    pub fn send_message_with_priority<C: Channel>(
        &mut self, message: M, priority: Priority
    ) {
        self.send.push((message, ChannelKind::of::<C>(), priority));
    }

    /// Buffers a message to be sent over the channel
    pub fn send_message<C: Channel>(
        &mut self, message: M
    ) {
        self.send.push((message, ChannelKind::of::<C>(), 1.0));
    }

    /// Take all messages from the MessageSender<M>, serialize them, and buffer them
    /// on the appropriate ChannelSender<C>
    ///
    /// SAFETY: the `message_sender` must be of type `MessageSender<M>`
    pub(crate) unsafe fn send_message_typed(
        message_sender: MutUntyped,
        transport: &Transport,
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY:  the `message_sender` must be of type `MessageSender<M>`
        let mut sender = unsafe { message_sender.with_type::<Self>()};
        // enable split borrows
        let sender = &mut *sender;
        sender.send.drain(..).try_for_each(|(message, channel_kind, priority)| {
            serialize_metadata.serialize::<M>(&message, &mut sender.writer, entity_map)?;
            let bytes = sender.writer.split();
            transport.send_erased(channel_kind, bytes, priority)?;
            Ok(())
        })
    }
}

impl MessagePlugin {
    // TODO: how can we maximize parallelism?
    //  - user can write raw bytes to ChannelSender<C> in parallel
    //  - users can buffer bytes to MessageSender<M> in parallel
    //
    // While sending we will:
    // - serialize all messages in parallel, and dump them in a Vec<(ChannelKind, Bytes, Priority)> that is shared
    //   across all MessageSenders<M>

    /// Take messages to send from the MessageSender<M> components
    /// Serialize them into bytes that are buffered in a ChannelSender<C>
    pub fn send(
        mut transport_query: Query<(Entity, &Transport, &mut MessageManager)>,
        // MessageSender<M> present on that entity
        mut message_sender_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
    ) {
        // We use Arc to make the query Clone, since we know that we will only access MessageSender<M> components
        // on different entities
        let mut message_sender_query = Arc::new(message_sender_query);
        transport_query.par_iter_mut().for_each(|(entity, transport, mut message_manager)| {
            // SAFETY: we know that this won't lead to violating the aliasing rule
            let mut message_sender_query = unsafe { message_sender_query.reborrow_unsafe() };
            // enable split borrows
            let message_manager = &mut *message_manager;
            message_manager.sender_ids.iter().try_for_each(|(message_kind, sender_id)| {
                let mut entity_mut = message_sender_query.get_mut(entity).unwrap();
                let message_sender = entity_mut.get_mut_by_id(*sender_id).ok_or(MessageError::MissingComponent(*sender_id))?;
                let send_metadata = registry.send_metadata.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                let serialize_fns = registry.serialize_fns_map.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                // SAFETY: we know the message_sender corresponds to the correct `MessageSender<M>` type
                unsafe { (send_metadata.send_message_fn)(
                    message_sender,
                    transport,
                    serialize_fns,
                    &mut message_manager.send_mapper,
                )?; }
                Ok::<_, MessageError>(())
            }).inspect_err(|e| error!("error sending message: {e:?}")).ok();
        })
    }
}