#![allow(dead_code)]
#![allow(unused)]

// re-exports
pub use bevy_ecs::prelude::Entity;
pub use enum_kinds::EnumKind;
pub use paste::paste;

pub use channel::channel::{
    Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
};
pub use connection::MessageManager;
pub use lightyear_derive::{
    component_protocol, message_protocol, Channel, ComponentProtocol, MessageProtocol,
};
pub use packet::message::{Message, MessageContainer};
pub use protocol::component::ComponentProtocol;
pub use protocol::message::MessageProtocol;
pub use protocol::{BitSerializable, Protocol};
pub use registry::channel::{ChannelKind, ChannelRegistry};
pub use serialize::reader::ReadBuffer;
pub use serialize::wordbuffer::reader::ReadWordBuffer;
pub use serialize::wordbuffer::writer::WriteWordBuffer;
pub use serialize::writer::WriteBuffer;
pub use transport::io::Io;
pub use transport::udp::UdpSocket;

pub mod channel;
mod connection;
mod events;
pub mod netcode;
pub mod packet;
pub(crate) mod protocol;
pub mod registry;
pub mod replication;
pub mod serialize;
pub mod transport;
