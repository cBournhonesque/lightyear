pub mod some_message {
    use bevy::ecs::entity::MapEntities;
    use bevy::prelude::{Entity, EntityMapper, Reflect};
    use serde::{Deserialize, Serialize};

    use lightyear::prelude::*;
    use lightyear_macros::{component_protocol, message_protocol};

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message1(pub u8);

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message2(Entity);

    impl MapEntities for Message2 {
        fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
            self.0 = entity_mapper.map_entity(self.0);
        }
    }

    #[message_protocol(protocol = "MyProtocol")]
    pub enum MyMessageProtocol {
        Message1(Message1),
        #[protocol(map_entities)]
        Message2(Message2),
    }

    #[component_protocol(protocol = "MyProtocol")]
    pub enum MyComponentProtocol {}

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
        BitSerializable, MessageProtocol, ReadBuffer, ReadWordBuffer, WriteBuffer, WriteWordBuffer,
    };

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
