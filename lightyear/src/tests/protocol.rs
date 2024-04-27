use bevy::ecs::entity::MapEntities;
use bevy::prelude::{default, Component, Entity, EntityMapper, Reflect, Resource};
use cfg_if::cfg_if;
use derive_more::{Add, Mul};
use std::ops::Mul;

use serde::{Deserialize, Serialize};

use crate::_internal::*;
use crate::prelude::*;

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct Message1(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct Message2(pub u32);

#[message_protocol_internal(protocol = "MyProtocol")]
pub enum MyMessageProtocol {
    Message1(Message1),
    Message2(Message2),
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul, Reflect)]
pub struct Component1(pub f32);

impl Mul<f32> for &Component1 {
    type Output = Component1;
    fn mul(self, rhs: f32) -> Self::Output {
        Component1(self.0 * rhs)
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul, Reflect)]
pub struct Component2(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul, Reflect)]
pub struct Component3(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component4(pub Entity);

impl MapEntities for Component4 {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.map_entity(self.0);
    }
}

#[component_protocol_internal(protocol = "MyProtocol")]
pub enum MyComponentsProtocol {
    #[protocol(sync(mode = "full"))]
    Component1(Component1),
    #[protocol(sync(mode = "simple"))]
    Component2(Component2),
    #[protocol(sync(mode = "once"))]
    Component3(Component3),
    #[protocol(sync(mode = "simple"), map_entities)]
    Component4(Component4),
    Resource1(ReplicateResource<Resource1>),
}

// Resources
#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone, Add, Reflect)]
pub struct Resource1(pub f32);

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Reflect)]
pub struct MyInput(pub i16);

impl UserAction for MyInput {}

// Protocol
cfg_if! {
    if #[cfg(feature = "leafwing")] {
        use leafwing_input_manager::Actionlike;
        #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
        pub enum LeafwingInput1 {
            Jump,
        }
        impl LeafwingUserAction for LeafwingInput1 {}

        #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
        pub enum LeafwingInput2 {
            Crouch,
        }
        impl LeafwingUserAction for LeafwingInput2 {}

        protocolize! {
            Self = MyProtocol,
            Message = MyMessageProtocol,
            Component = MyComponentsProtocol,
            Input = MyInput,
            LeafwingInput1 = LeafwingInput1,
            LeafwingInput2 = LeafwingInput2,
            Crate = crate,
        }
    } else {
        protocolize! {
            Self = MyProtocol,
            Message = MyMessageProtocol,
            Component = MyComponentsProtocol,
            Input = MyInput,
            Crate = crate,
        }
    }
}

// Channels
#[derive(ChannelInternal, Reflect)]
pub struct Channel1;

#[derive(ChannelInternal, Reflect)]
pub struct Channel2;

pub fn protocol() -> MyProtocol {
    let mut p = MyProtocol::default();
    p.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliable,
        ..default()
    });
    p.add_channel::<Channel2>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliableWithAcks,
        ..default()
    });
    p
}
