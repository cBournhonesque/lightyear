#![allow(dead_code)]
#![allow(unused)]

// re-exports
pub use bevy_app::prelude::App;
pub use bevy_app::prelude::PostUpdate;
pub use bevy_ecs::prelude::Entity;
pub use bevy_ecs::world::{EntityMut, World};
pub use enum_delegate;
pub use enum_dispatch::enum_dispatch;
pub use paste::paste;

pub use channel::channel::{
    Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
    ReliableSettings,
};
pub use config::SharedConfig;
pub use connection::{Connection, ConnectionEvents};
pub use lightyear_derive::{component_protocol, message_protocol, Channel, MessageProtocol};
pub use packet::message::{Message, MessageContainer};
pub use packet::message_manager::MessageManager;
pub use plugin::{ReplicationData, ReplicationSet, SharedPlugin};
pub use protocol::channel::{ChannelKind, ChannelRegistry};
pub use protocol::component::{ComponentBehaviour, ComponentProtocol, ComponentProtocolKind};
pub use protocol::message::MessageProtocol;
pub use protocol::{BitSerializable, Protocol};
pub use replication::DefaultReliableChannel;
pub use replication::ReplicationSend;
pub use serialize::reader::ReadBuffer;
pub use serialize::wordbuffer::reader::ReadWordBuffer;
pub use serialize::wordbuffer::writer::WriteWordBuffer;
pub use serialize::writer::WriteBuffer;
pub use transport::io::{Io, IoConfig};
pub use transport::udp::UdpSocket;

pub mod channel;
mod config;
mod connection;
pub mod netcode;
pub mod packet;
pub mod plugin;
pub mod protocol;
pub mod replication;
pub mod serialize;
pub mod transport;
