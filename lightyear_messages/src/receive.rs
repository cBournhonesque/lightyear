use crate::plugin::MessagePlugin;
use crate::registry::{MessageError, MessageRegistry};
use crate::MessageManager;
use crate::{Message, MessageNetId};
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::world::FilteredEntityMut;
use bevy::prelude::{Component, Entity, Query, Res};
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::prelude::Transport;
use tracing::{error, trace};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use alloc::sync::Arc;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::packet::message::MessageId;

#[derive(Component)]
#[require(MessageManager)]
pub struct MessageReceiver<M> {
    // TODO: wrap this in bevy events buffer?
    recv: Vec<ReceivedMessage<M>>
}

#[derive(Debug)]
pub struct ReceivedMessage<M> {
    pub data: M,
    /// Tick on the remote peer when the message was sent,
    pub remote_tick: Tick,
    /// Channel that was used to send the message
    pub channel_kind: ChannelKind,
    /// MessageId of the message
    pub message_id: Option<MessageId>,
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
        self.recv.drain(..).map(|m| m.data)
    }

    /// Take all messages from the MessageReceiver<M>, deserialize them, and return them
    pub fn receive_with_tick(&mut self) -> impl Iterator<Item=ReceivedMessage<M>> {
        self.recv.drain(..)
    }
}

pub(crate) type ReceiveMessageFn = unsafe fn(
    receiver: MutUntyped,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
) -> Result<(), MessageError>;

impl<M: Message> MessageReceiver<M> {

    /// Receive a single message of type `M` from the channel
    ///
    /// SAFETY: the `receiver` must be of type `MessageReceiver<M>`, and the `message_bytes` must be a valid serialized message of type `M`
    pub(crate) unsafe fn receive_message_typed(
        receiver: MutUntyped,
        reader: &mut Reader,
        channel_kind: ChannelKind,
        remote_tick: Tick,
        message_id: Option<MessageId>,
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>()};
        // we deserialize the message and send a MessageEvent
        let message = unsafe { serialize_metadata.deserialize::<M>(reader, entity_map)? };
        let received_message = ReceivedMessage {
            data: message,
            remote_tick,
            channel_kind,
            message_id,
        };
        trace!("Pushing message {:?} on channel {channel_kind:?}", core::any::type_name::<M>());
        receiver.recv.push(received_message);
        Ok(())
    }
}

impl MessagePlugin {
    /// Receive bytes from each channel of the Transport
    /// Deserialize the bytes into Messages that are buffered in the MessageReceiver<M> component
    pub fn recv(
        // NOTE: we only need the mut bound on MessageManager because EntityMapper requires mut
        mut transport_query: Query<(Entity, &mut MessageManager, &mut Transport)>,
        // List of ChannelReceivers<M> present on that entity
        receiver_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
    ) {
        // We use Arc to make the query Clone, since we know that we will only access MessagerReceiver<M> components
        // on different entities
        let receiver_query = Arc::new(receiver_query);
        transport_query.par_iter_mut().for_each(|(entity, mut message_manager, mut transport)| {
            // SAFETY: we know that this won't lead to violating the aliasing rule
            let mut receiver_query = unsafe { receiver_query.reborrow_unsafe() };
            // enable split borrows
            let transport = &mut *transport;
            // TODO: we can run this in parallel using rayon!
            transport.receivers.values_mut().try_for_each(|receiver_metadata| {
                let channel_kind = receiver_metadata.channel_kind;
                while let Some((tick, bytes, message_id)) = receiver_metadata.receiver.read_message() {
                    trace!("Received message {message_id:?} on channel {channel_kind:?}");
                    let mut reader = Reader::from(bytes);
                    // we receive the message NetId, and then deserialize the message
                    let message_net_id = MessageNetId::from_bytes(&mut reader)?;
                    let message_kind = registry.kind_map.kind(message_net_id).ok_or(MessageError::UnrecognizedMessageId(message_net_id))?;
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
                        &mut reader,
                        channel_kind,
                        tick,
                        message_id,
                        serialize_fns,
                        &mut message_manager.entity_mapper.remote_to_local
                    )?; }
                }
                Ok::<_, MessageError>(())
            }).inspect_err(|e| error!("Error receiving messages: {e:?}")).ok();
        })
    }

}