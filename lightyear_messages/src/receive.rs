use crate::plugin::MessagePlugin;
use crate::registry::serialize::ErasedSerializeFns;
use crate::registry::{MessageError, MessageRegistry};
use crate::MessageManager;
use crate::{Message, MessageId};
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::world::FilteredEntityMut;
use bevy::prelude::{Component, Entity, Query, Res};
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_serde::reader::Reader;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::entity_map::ReceiveEntityMap;
use lightyear_transport::prelude::Transport;
use tracing::error;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use alloc::sync::Arc;

#[derive(Component)]
#[require(MessageManager)]
pub struct MessageReceiver<M> {
    recv: Vec<(M, ChannelKind, Tick)>
}


impl<M> Default for MessageReceiver<M> {
    fn default() -> Self {
        Self {
            recv: Vec::new(),
        }
    }
}

// TODO: do we care about the channel that the message was sent from? user-specified message usually don't
// TODO: we have access to the Tick, so we could decide at which timeline we want to receive the message!
impl<M: Message> MessageReceiver<M> {
    /// Take all messages from the MessageReceiver<M>, deserialize them, and return them
    pub fn receive(&mut self) -> impl Iterator<Item=M>{
        self.recv.drain(..).map(|(message, _, _)| message)
    }
}

pub(crate) type ReceiveMessageFn = unsafe fn(
    receiver: MutUntyped,
    message_bytes: (Reader, ChannelKind, Tick),
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
) -> Result<(), MessageError>;

impl<M: Message> MessageReceiver<M> {

    /// Receive a single message of type `M` from the channel
    ///
    /// SAFETY: the `receiver` must be of type `MessageReceiver<M>`, and the `message_bytes` must be a valid serialized message of type `M`
    pub(crate) unsafe fn receive_message_typed(
        receiver: MutUntyped,
        mut message_bytes: (Reader, ChannelKind, Tick),
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>()};
        // we deserialize the message and send a MessageEvent
        let message = unsafe { serialize_metadata.deserialize::<M>(&mut message_bytes.0, entity_map)? };
        receiver.recv.push((message, message_bytes.1, message_bytes.2));
        Ok(())
    }
}

impl MessagePlugin {
    /// Receive bytes from each channel of the Transport
    /// Deserialize the bytes into Messages that are buffered in the MessageReceiver<M> component
    pub fn recv(
        mut transport_query: Query<(Entity, &mut Transport)>,
        // List of ChannelReceivers<M> present on that entity
        receiver_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
    ) {
        // We use Arc to make the query Clone, since we know that we will only access MessagerReceiver<M> components
        // on different entities
        let receiver_query = Arc::new(receiver_query);
        transport_query.par_iter_mut().for_each(|(entity, mut transport)| {
            // SAFETY: we know that this won't lead to violating the aliasing rule
            let mut receiver_query = unsafe { receiver_query.reborrow_unsafe() };
            // enable split borrows
            let transport = &mut *transport;
            // TODO: maybe store using channel-kind? channel-id is a serialization-specific id, which should
            //  remain in lightyear_transport
            transport.receivers.values_mut().try_for_each(|receiver_metadata| {
                let channel_kind = receiver_metadata.channel_kind;
                // TODO: maybe probide ChannelKind?
                while let Some((tick, bytes)) = receiver_metadata.receiver.read_message() {
                    let mut reader = Reader::from(bytes);
                    // we receive the message NetId, and then deserialize the message
                    let message_id = MessageId::from_bytes(&mut reader)?;
                    let message_kind = registry.kind_map.kind(message_id).ok_or(MessageError::UnrecognizedMessageId(message_id))?;
                    let recv_metadata = registry.receive_metadata.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                    let component_id = recv_metadata.component_id;
                    let mut entity_mut = receiver_query.get_mut(entity).unwrap();
                    let receiver = entity_mut
                        .get_mut_by_id(component_id)
                        .ok_or(MessageError::MissingComponent(component_id))?;

                    let serialize_fns = registry.serialize_fns_map.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                    // SAFETY: we know the receiver corresponds to the correct `MessageReceiver<M>` type
                    unsafe { (recv_metadata.receive_message_fn)(
                        receiver,
                        (reader, channel_kind, tick),
                        serialize_fns,
                        &mut transport.recv_mapper
                    )?; }
                }
                Ok::<_, MessageError>(())
            }).inspect_err(|e| error!("Error receiving messages: {e:?}")).ok();
        })
    }

}