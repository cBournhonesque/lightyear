pub mod some_component {
    use bevy::ecs::entity::MapEntities;
    use bevy::prelude::{Component, Entity, EntityMapper, Reflect};
    use derive_more::Add;
    use serde::{Deserialize, Serialize};
    use std::ops::Mul;

    use lightyear::prelude::client::LerpFn;
    use lightyear::prelude::*;
    use lightyear_macros::{component_protocol, message_protocol};

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Add, Reflect)]
    pub struct Component1(pub f32);

    impl Mul<f32> for &Component1 {
        type Output = Component1;
        fn mul(self, rhs: f32) -> Self::Output {
            Component1(self.0 * rhs)
        }
    }

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Component2(pub(crate) Entity);

    impl MapEntities for Component2 {
        fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
            self.0 = entity_mapper.map_entity(self.0);
        }
    }

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Component3(String);

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Component4(String);

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Component5(pub f32);

    #[component_protocol(protocol = "MyProtocol")]
    pub enum MyComponentProtocol {
        #[protocol(sync(mode = "full", lerp = "LinearInterpolator"))]
        Component1(Component1),
        #[protocol(sync(mode = "simple"), map_entities)]
        Component2(Component2),
        #[protocol(sync(mode = "once"))]
        Component3(Component3),
        Component4(Component4),
        #[protocol(sync(mode = "full", lerp = "MyCustomInterpolator"))]
        Component5(Component5),
    }

    // custom interpolation logic
    pub struct MyCustomInterpolator;
    impl<C: Clone> LerpFn<C> for MyCustomInterpolator {
        fn lerp(start: &C, _other: &C, _t: f32) -> C {
            start.clone()
        }
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message1(pub u32);

    #[message_protocol(protocol = "MyProtocol")]
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
    use bevy::ecs::entity::MapEntities;
    use bevy::prelude::Entity;
    use lightyear::_reexport::ComponentProtocol;
    use lightyear::prelude::RemoteEntityMap;
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

        // check interpolation
        let component5 = Component5(0.1);
        assert_eq!(
            component5.clone(),
            MyComponentProtocol::lerp(&component5, &Component5(0.0), 0.5)
        );

        let component1 = Component1(0.0);
        assert_eq!(
            Component1(0.5),
            MyComponentProtocol::lerp(&component1, &Component1(1.0), 0.5)
        );

        let mut mapper = RemoteEntityMap::default();
        let remote_entity = Entity::from_raw(0);
        let local_entity = Entity::from_raw(1);
        mapper.insert(remote_entity, local_entity);
        let component2 = Component2(remote_entity);
        let mut protocol_component2 = MyComponentProtocol::Component2(component2.clone());
        protocol_component2.map_entities(&mut mapper);
        let mapped_component2: Component2 = protocol_component2.try_into().unwrap();
        assert_eq!(mapped_component2.0, local_entity);

        Ok(())
    }
}
