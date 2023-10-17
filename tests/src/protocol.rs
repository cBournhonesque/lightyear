use bevy::prelude::Component;
use serde::{Deserialize, Serialize};

use lightyear_shared::channel::channel::ReliableSettings;
use lightyear_shared::Protocol;
use lightyear_shared::{component_protocol, message_protocol};
use lightyear_shared::{protocolize, Channel};
use lightyear_shared::{ChannelDirection, ChannelMode, ChannelSettings};

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

#[derive(Debug, PartialEq)]
#[message_protocol]
pub enum MyMessageProtocol {
    Message1(Message1),
    Message2(Message2),
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component1;

#[derive(Debug, PartialEq)]
#[component_protocol]
pub enum MyComponentsProtocol {
    Component1(Component1),
}

protocolize!(MyProtocol, MyMessageProtocol, MyComponentsProtocol);

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
