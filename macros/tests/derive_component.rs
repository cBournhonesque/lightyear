pub mod some_component {
    use bevy::ecs::entity::MapEntities;
    use bevy::prelude::{Component, EntityMapper};
    use derive_more::Add;
    use serde::{Deserialize, Serialize};
    use std::ops::Mul;

    use lightyear::prelude::client::LerpFn;
    use lightyear::prelude::*;
    use lightyear::shared::replication::entity_map::Mapper;
    use lightyear_macros::{component_protocol, message_protocol};

    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone, Add)]
    pub struct Component1(pub f32);

    impl Mul<f32> for &Component1 {
        type Output = Component1;
        fn mul(self, rhs: f32) -> Self::Output {
            Component1(self.0 * rhs)
        }
    }

    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Component2(pub f32);

    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Component3(String);

    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Component4(String);

    #[derive(Component, Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Component5(pub f32);

    #[component_protocol(protocol = "MyProtocol")]
    pub enum MyComponentProtocol {
        #[protocol(sync(mode = "full", lerp = "NullInterpolator"), map)]
        Component1(Component1),
        #[protocol(sync(mode = "simple"))]
        Component2(Component2),
        #[protocol(sync(mode = "once"))]
        Component3(Component3),
        Component4(Component4),
        #[protocol(sync(mode = "full", lerp = "MyCustomInterpolator"))]
        Component5(Component5),
    }

    impl<C: MapEntities> Mapper<C> for MyComponentProtocol {
        fn map_entities<M: EntityMapper>(component: &mut C, entity_mapper: &mut M) {
            component.map_entities(entity_mapper)
        }
    }

    impl MapEntities for MyComponentProtocol {
        fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
            match self {
                MyComponentProtocol::Component1(x) => {
                    <Self as Mapper<Component1>>::map_entities(x, entity_mapper)
                }
                MyComponentProtocol::Component2(_) => {}
                MyComponentProtocol::Component3(_) => {}
                MyComponentProtocol::Component4(_) => {}
                MyComponentProtocol::Component5(_) => {}
                MyComponentProtocol::ShouldBePredicted(_) => {}
                MyComponentProtocol::PrePredicted(_) => {}
                MyComponentProtocol::PreSpawnedPlayerObject(_) => {}
                MyComponentProtocol::ShouldBeInterpolated(_) => {}
                MyComponentProtocol::ParentSync(_) => {}
            }
        }
    }

    // custom interpolation logic
    pub struct MyCustomInterpolator;
    impl<C> LerpFn<C> for MyCustomInterpolator {
        fn lerp(start: &C, _other: &C, _t: f32) -> C {
            start
        }
    }

    #[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
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
    use lightyear::_reexport::ComponentProtocol;
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

        Ok(())
    }
}
