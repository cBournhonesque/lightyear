use bevy::prelude::Component;
use derive_more::{Add, Mul};
use serde::{Deserialize, Serialize};

use lightyear::prelude::*;

// Messages
#[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

#[message_protocol(protocol = "MyProtocol", derive(Debug))]
pub enum MyMessageProtocol {
    Message1(Message1),
    Message2(Message2),
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul)]
pub struct Component1(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul)]
pub struct Component2(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul)]
pub struct Component3(pub f32);

#[component_protocol(protocol = "MyProtocol", derive(Debug))]
pub enum MyComponentsProtocol {
    #[sync(full)]
    Component1(Component1),
    #[sync(simple)]
    Component2(Component2),
    #[sync(once)]
    Component3(Component3),
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
}

// Channels
#[derive(Channel)]
pub struct Channel1;

#[derive(Channel)]
pub struct Channel2;

pub fn protocol() -> MyProtocol {
    let mut p = MyProtocol::default();
    p.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        direction: ChannelDirection::Bidirectional,
    });
    p.add_channel::<Channel2>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliable,
        direction: ChannelDirection::Bidirectional,
    });
    p
}
