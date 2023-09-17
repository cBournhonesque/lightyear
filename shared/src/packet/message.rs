use bytes::Bytes;

/// A Message is a logical unit of data that should be transmitted over a network
///
/// The message can be small (multiple messages can be sent in a single packet)
/// or big (a single message can be split between multiple packets)
///
/// A Message knows how to serialize itself (messageType + Data)
/// and knows how many bits it takes to serialize itself
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    // kind
    data: Bytes,
}

impl Message {
    // fn kind(&self) -> MessageKind {
    //     unimplemented!()
    // }

    pub fn new(data: Bytes) -> Self {
        Message { data }
    }

    /// Bit length of the serialized message (including the message id and message kind)
    pub fn bit_len(&self) -> u32 {
        unimplemented!()
    }

    fn to_bytes(&self) -> Bytes {
        // TODO: use bitcode to serialize?
        unimplemented!()
    }
}
