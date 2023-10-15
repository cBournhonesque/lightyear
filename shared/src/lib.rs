#![allow(dead_code)]
#![allow(unused)]

// re-exports
pub use bevy_ecs::prelude::Entity;
pub use enum_kinds::EnumKind;
pub use paste::paste;

pub use channel::channel::{
    Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
};
pub use connection::{Connection, Events};
pub use lightyear_derive::{
    component_protocol, message_protocol, Channel, ComponentProtocol, ComponentProtocolKind,
    MessageProtocol,
};
pub use packet::message::{Message, MessageContainer};
pub use packet::message_manager::MessageManager;
pub use protocol::channel::{ChannelKind, ChannelRegistry};
pub use protocol::component::ComponentProtocol;
pub use protocol::component::ComponentProtocolKind;
pub use protocol::message::MessageProtocol;
pub use protocol::{BitSerializable, Protocol};
pub use serialize::reader::ReadBuffer;
pub use serialize::wordbuffer::reader::ReadWordBuffer;
pub use serialize::wordbuffer::writer::WriteWordBuffer;
pub use serialize::writer::WriteBuffer;
pub use transport::io::Io;
pub use transport::udp::UdpSocket;

pub mod channel;
mod connection;
pub mod netcode;
pub mod packet;
pub(crate) mod protocol;
pub mod replication;
pub mod serialize;
pub mod transport;
