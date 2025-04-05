use crate::plugin::MessagePlugin;
use crate::registry::serialize::ErasedSerializeFns;
use crate::registry::{MessageError, MessageKind, MessageRegistry};
use crate::{Message, MessageId, MessageManager};
use alloc::sync::Arc;
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::{DeferredWorld, FilteredEntityMut};
use bevy::prelude::{Component, Entity, Query, Res, Without, World};
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::writer::Writer;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::{Channel, ChannelKind};
use lightyear_transport::prelude::Transport;
use tracing::{debug, error, trace};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

pub type Priority = f32;

#[derive(Component)]
#[component(on_add = MessageSender::<M>::on_add_hook)]
#[require(MessageManager)]
pub struct MessageSender<M: Message> {
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
    message_id: MessageId,
    transport: &Transport,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut SendEntityMap,
) -> Result<(), MessageError>;


impl<M: Message> MessageSender<M> {

    /// Buffers a message to be sent over the channel
    pub fn send_with_priority<C: Channel>(
        &mut self, message: M, priority: Priority
    ) {
        self.send.push((message, ChannelKind::of::<C>(), priority));
    }

    /// Buffers a message to be sent over the channel
    pub fn send<C: Channel>(
        &mut self, message: M
    ) {
        trace!(message = ?core::any::type_name::<M>(), channel = ?core::any::type_name::<C>(), "Sending message");
        self.send.push((message, ChannelKind::of::<C>(), 1.0));
    }

    /// Take all messages from the MessageSender<M>, serialize them, and buffer them
    /// on the appropriate ChannelSender<C>
    ///
    /// SAFETY: the `message_sender` must be of type `MessageSender<M>`
    pub(crate) unsafe fn send_message_typed(
        message_sender: MutUntyped,
        message_id: MessageId,
        transport: &Transport,
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY:  the `message_sender` must be of type `MessageSender<M>`
        let mut sender = unsafe { message_sender.with_type::<Self>()};
        // enable split borrows
        let sender = &mut *sender;
        sender.send.drain(..).try_for_each(|(message, channel_kind, priority)| {
            // we write the message NetId, and then serialize the message
            message_id.to_bytes(&mut sender.writer)?;
            serialize_metadata.serialize::<M>(&message, &mut sender.writer, entity_map)?;
            let bytes = sender.writer.split();
            transport.send_erased(channel_kind, bytes, priority)?;
            Ok(())
        })
    }

    pub fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            // let registry = world.resource::<MessageRegistry>();
            // TODO: should we verify that the message has been registered?
            // let Some(sender_id) = registry.send_metadata.get(&MessageKind::of::<M>()).map(|metadata| metadata.component_id) else {
            //     // TODO: should we just re-register the message if it's not registered?
            //     panic!("Message {M} must be registered to the MessageRegistry");
            // };
            world
                .entity_mut(context.entity)
                .get_mut::<MessageManager>()
                .unwrap()
                .sender_ids
                .insert(MessageKind::of::<M>(), context.component_id);
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
        // TODO: maybe prevent users from sending messages if Connecting/Disconnected is present?
        //   but then this crate would import lightyear_connection; and we might want to remain independent
        //   or should we have a feature called 'connection'?
        mut transport_query: Query<(Entity, &Transport, &mut MessageManager)>,
        // MessageSender<M> present on that entity
        message_sender_query: Query<FilteredEntityMut>,
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
                let message_id = registry.kind_map.net_id(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                // SAFETY: we know the message_sender corresponds to the correct `MessageSender<M>` type
                unsafe { (send_metadata.send_message_fn)(
                    message_sender,
                    *message_id,
                    transport,
                    serialize_fns,
                    &mut message_manager.send_mapper,
                )?; }
                Ok::<_, MessageError>(())
            }).inspect_err(|e| error!("error sending message: {e:?}")).ok();
        })
    }
}
