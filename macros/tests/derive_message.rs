pub mod some_message {
    use bevy::prelude::Component;
    use serde::{Deserialize, Serialize};

    use lightyear::prelude::*;
    use lightyear_macros::{component_protocol, message_protocol};

    #[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message1(pub u8);

    #[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message2(pub u32);

    #[message_protocol(protocol = "MyProtocol")]
    pub enum MyMessageProtocol {
        Message1(Message1),
        Message2(Message2),
    }

    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
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
    use super::some_message::*;
    use lightyear::_reexport::{
        BitSerializable, ReadBuffer, ReadWordBuffer, WriteBuffer, WriteWordBuffer,
    };
    use lightyear::prelude::*;

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

        Ok(())
    }
}
