use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::MessageContainer;

pub trait MessageProtocol {
    type Enum;

    fn encode(&self, writer: &mut impl WriteBuffer);

    fn decode(&self, reader: &mut impl ReadBuffer) -> anyhow::Result<MessageContainer<Self::Enum>>;
    //     fn decode(
    //         &self,
    //         registry: &MessageRegistry,
    //         reader: &mut impl ReadBuffer,
    //     ) -> anyhow::Result<MessageContainer>;
}

// client writes an Enum containing all their message type
// each message must derive message

// that big enum will implement MessageProtocol via a proc macro
