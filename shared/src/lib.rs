#![allow(dead_code)]
#![allow(unused)]

pub use channel::channel::{
    Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
};
pub use connection::Connection;
pub use lightyear_derive::Channel;
pub use packet::message::{Message, MessageContainer};
pub use protocol::{Protocol, SerializableProtocol};
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
