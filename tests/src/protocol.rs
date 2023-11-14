use bevy::prelude::Component;
use serde::{Deserialize, Serialize};

use lightyear_shared::channel::channel::ReliableSettings;
use lightyear_shared::{component_protocol, message_protocol};
use lightyear_shared::{protocolize, Channel, Message};
use lightyear_shared::{ChannelDirection, ChannelMode, ChannelSettings};
use lightyear_shared::{Protocol, UserInput};

// Messages
#[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

#[message_protocol(protocol = "MyProtocol")]
pub enum MyMessageProtocol {
    Message1(Message1),
    Message2(Message2),
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Component1;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Component2;

#[component_protocol(protocol = "MyProtocol")]
pub enum MyComponentsProtocol {
    #[replication(predicted)]
    Component1(Component1),
    Component2(Component2),
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Input(pub usize);
impl UserInput for Input {}

// Protocol

protocolize! {
    Self = MyProtocol,
    Message = MyMessageProtocol,
    Component = MyComponentsProtocol,
    Input = Input,
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
