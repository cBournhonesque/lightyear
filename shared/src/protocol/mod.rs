use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;

pub(crate) mod channel;
pub(crate) mod message;

pub trait Protocol {
    type Message: SerializableProtocol;
    type Channel: channel::ChannelProtocol;

    fn get_message_protocol() -> Self::Message;
    fn get_channel_protocol() -> Self::Channel;
}

/// A protocol that can be serialized through channels
pub trait SerializableProtocol: Clone {
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()>;

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized;
    //     fn decode(
    //         &self,
    //         registry: &MessageRegistry,
    //         reader: &mut impl ReadBuffer,
    //     ) -> anyhow::Result<MessageContainer>;
}

// TODO: if this all we need from message protocol, then use this!
//  then we can have messageProtocols require to implement a last marker trait called IsMessageProtocol

impl<'a, T> SerializableProtocol for T
where
    T: Serialize + DeserializeOwned + Clone,
{
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        writer.serialize(self)
    }

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        reader.deserialize::<Self>()
    }
}

// #[cfg(test)]
// impl<'a> SerializableProtocol for i32 {
//     fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
//         writer.serialize(self)
//     }
//
//     fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
//     where
//         Self: Sized,
//     {
//         reader.deserialize::<Self>()
//     }
// }
