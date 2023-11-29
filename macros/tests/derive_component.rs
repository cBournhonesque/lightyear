pub mod some_component {
    use bevy::prelude::Component;
    use derive_more::{Add, Mul};
    use serde::{Deserialize, Serialize};

    use lightyear::prelude::*;
    use lightyear_macros::{component_protocol, message_protocol};

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Add, Mul)]
    pub struct Component1(pub f32);

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Add, Mul)]
    pub struct Component2(pub f32);

    #[component_protocol(protocol = "MyProtocol", derive(Debug))]
    pub enum MyComponentProtocol {
        #[sync(full)]
        Component1(Component1),
        #[sync(simple)]
        Component2(Component2),
    }

    #[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message1(pub u32);

    #[message_protocol(protocol = "MyProtocol")]
    #[derive(Debug)]
    pub enum MyMessageProtocol {
        Message1(Message1),
    }

    protocolize! {
        Self = MyProtocol,
        Message = MyMessageProtocol,
        Component = MyComponentProtocol,
    }
}

#[cfg(test)]
mod tests {
    use lightyear::protocol::BitSerializable;
    use lightyear::serialize::reader::ReadBuffer;
    use lightyear::serialize::wordbuffer::reader::ReadWordBuffer;
    use lightyear::serialize::wordbuffer::writer::WriteWordBuffer;
    use lightyear::serialize::writer::WriteBuffer;

    use super::some_component::*;

    #[test]
    fn test_component_derive() -> anyhow::Result<()> {
        let component1: MyComponentProtocol = MyComponentProtocol::Component1(Component1(1.0));
        let mut writer = WriteWordBuffer::with_capacity(10);
        component1.encode(&mut writer)?;
        let bytes = writer.finish_write();

        let mut reader = ReadWordBuffer::start_read(bytes);
        let copy = MyComponentProtocol::decode(&mut reader)?;
        assert_eq!(component1, copy);

        Ok(())
    }
}
