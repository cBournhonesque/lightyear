use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};

use lightyear_shared::channel::channel::ReliableSettings;
use lightyear_shared::Channel;
use lightyear_shared::Protocol;
use lightyear_shared::{ChannelDirection, ChannelMode, ChannelRegistry, ChannelSettings};

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum MyMessageProtocol {
    Message1(Message1),
    Message2(Message2),
}

pub enum MyProtocol {
    MyMessageProtocol(MyMessageProtocol),
}

impl Protocol for MyProtocol {
    type Message = MyMessageProtocol;
}

// Channels
#[derive(Channel)]
pub struct Channel1;

#[derive(Channel)]
pub struct Channel2;

lazy_static! {
    pub static ref CHANNEL_REGISTRY: ChannelRegistry = {
        let mut c = ChannelRegistry::new();
        c.add::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            direction: ChannelDirection::Bidirectional,
        })
        .unwrap();
        c.add::<Channel2>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        })
        .unwrap();
        c
    };
}
