use bevy::prelude::{Component, Entity};
use bevy::utils::EntityHashSet;
use derive_more::{Add, Mul};
use serde::{Deserialize, Serialize};

use crate::_reexport::*;
use crate::prelude::*;

// Messages
#[derive(MessageInternal, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(MessageInternal, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

#[message_protocol_internal(protocol = "MyProtocol")]
pub enum MyMessageProtocol {
    Message1(Message1),
    Message2(Message2),
}

// Components
#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul)]
pub struct Component1(pub f32);

#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul)]
pub struct Component2(pub f32);

#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul)]
pub struct Component3(pub f32);

#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[message(custom_map)]
pub struct Component4(pub Entity);

impl<'a> MapEntities<'a> for Component4 {
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {
        self.0.map_entities(entity_mapper);
    }

    fn entities(&self) -> EntityHashSet<Entity> {
        EntityHashSet::from_iter(vec![self.0])
    }
}

#[component_protocol_internal(protocol = "MyProtocol")]
pub enum MyComponentsProtocol {
    #[sync(full)]
    Component1(Component1),
    #[sync(simple)]
    Component2(Component2),
    #[sync(once)]
    Component3(Component3),
    #[sync(simple)]
    Component4(Component4),
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct MyInput(pub i16);

impl UserInput for MyInput {}

// Protocol
protocolize! {
    Self = MyProtocol,
    Message = MyMessageProtocol,
    Component = MyComponentsProtocol,
    Input = MyInput,
    Crate = crate,
}

// Channels
#[derive(ChannelInternal)]
pub struct Channel1;

#[derive(ChannelInternal)]
pub struct Channel2;

pub fn protocol() -> MyProtocol {
    let mut p = MyProtocol::default();
    p.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliable,
        direction: ChannelDirection::Bidirectional,
    });
    p.add_channel::<Channel2>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliableWithAcks,
        direction: ChannelDirection::Bidirectional,
    });
    p
}
