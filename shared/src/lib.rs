#![allow(dead_code)]
#![allow(unused)]

pub use channel::channel::{
    Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
};
pub use connection::Connection;
pub use packet::message::{Message, MessageContainer};
pub use protocol::{Protocol, SerializableProtocol};
pub use registry::channel::{ChannelKind, ChannelRegistry};
pub use serialize::reader::ReadBuffer;
pub use serialize::wordbuffer::reader::ReadWordBuffer;
pub use serialize::wordbuffer::writer::WriteWordBuffer;
pub use serialize::writer::WriteBuffer;

pub mod channel;
mod connection;
pub mod netcode;
pub mod packet;
pub(crate) mod protocol;
pub mod registry;
pub mod serialize;
mod transport;
