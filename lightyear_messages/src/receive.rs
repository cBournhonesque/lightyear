use crate::registry::serialize::ErasedSerializeFns;
use crate::registry::MessageError;
use crate::Message;
use bevy::prelude::Component;
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_serde::reader::Reader;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::entity_map::ReceiveEntityMap;

#[derive(Component)]
#[require(MessageManager)]
pub struct MessageReceiver<M> {
    recv: Vec<(M, ChannelKind, Tick)>
}


impl<M: Message> MessageReceiver<M> {

    /// Receive a single message of type `M` from the channel
    pub(crate) fn receive_message_typed<M>(
        &mut self,
        message_bytes: (Bytes, ChannelKind, Tick),
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<M, MessageError> {
        let reader = &mut Reader::from(message_bytes.0);
        // we deserialize the message and send a MessageEvent
        let message = unsafe { serialize_metadata.deserialize::<M>(reader, entity_map)? };
        self.recv.push((message, message_bytes.1, message_bytes.2));
        Ok(())
    }
}