pub mod some_message {
    use bevy::prelude::Component;
    use bitcode::{Decode, Encode};
    use serde::{Deserialize, Serialize};

    use lightyear_derive::{component_protocol, message_protocol};
    use lightyear_shared::{protocolize, Message, Named};

    #[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message1(pub u8);

    #[derive(Message, Encode, Decode, Debug, PartialEq, Clone)]
    #[serialize(bitcode)]
    #[bitcode_hint(gamma)]
    pub struct Message2(pub u32);

    #[derive(Debug, PartialEq, Clone, Serialize, Deserialize, Debug, PartialEq)]
    #[message_protocol(protocol = "MyProtocol")]
    // #[derive(EnumAsInner)]
    pub enum MyMessageProtocol {
        Message1(Message1),
        Message2(Message2),
    }

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Component1(pub u8);

    #[component_protocol(protocol = "MyProtocol")]
    pub enum MyComponentProtocol {
        Component1(Component1),
    }

    protocolize! {
        Self = MyProtocol,
        Message = MyMessageProtocol,
        Component = MyComponentProtocol,
    }
}

#[cfg(test)]
mod tests {
    use lightyear_shared::BitSerializable;
    use lightyear_shared::{Named, ReadBuffer, ReadWordBuffer, WriteBuffer, WriteWordBuffer};

    use crate::some_message::MyMessageProtocol;

    use super::some_message::*;

    #[test]
    fn test_message_derive() -> anyhow::Result<()> {
        let message1: MyMessageProtocol = MyMessageProtocol::Message1(Message1(1));
        assert_eq!(message1.name(), "Message1");
        let mut writer = WriteWordBuffer::with_capacity(10);
        message1.encode(&mut writer)?;
        let bytes = writer.finish_write();

        let mut reader = ReadWordBuffer::start_read(bytes);
        let copy = MyMessageProtocol::decode(&mut reader)?;
        assert_eq!(message1, copy);

        // check that message 2 is encoded with bitcode

        Ok(())
    }
}
