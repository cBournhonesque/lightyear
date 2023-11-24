#![allow(dead_code)]
#![allow(unused)]

extern crate core;

// re-exports
pub use bevy::ecs::world::EntityMut;
pub use bevy::prelude::{App, Entity, PostUpdate, World};
pub use enum_as_inner::EnumAsInner;
pub use enum_delegate;
pub use enum_dispatch::enum_dispatch;
pub use paste::paste;

pub use channel::channel::{
    Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
    DefaultUnorderedUnreliableChannel, EntityActionsChannel, EntityUpdatesChannel, InputChannel,
    PingChannel, ReliableSettings,
};
pub use client::Client;
pub use connection::{Connection, ConnectionEvents};
pub use inputs::input_buffer::{InputMessage, UserInput};
pub use lightyear_derive::{component_protocol, message_protocol, Channel, Message};
pub use netcode::ClientId;
pub use packet::message::{Message, MessageContainer};
pub use packet::message_manager::MessageManager;
pub use plugin::config::SharedConfig;
pub use plugin::events::{ConnectEvent, DisconnectEvent, EntitySpawnEvent};
pub use plugin::sets::{MainSet, ReplicationSet};
pub use plugin::{ReplicationData, SharedPlugin};
pub use protocol::channel::{ChannelKind, ChannelRegistry};
pub use protocol::component::{
    ComponentBehaviour, ComponentKindBehaviour, ComponentProtocol, ComponentProtocolKind, IntoKind,
};
pub use protocol::message::{MessageBehaviour, MessageKind, MessageProtocol};
pub use protocol::{BitSerializable, Protocol};
pub use replication::ReplicationSend;
pub use serialize::reader::ReadBuffer;
pub use serialize::wordbuffer::reader::ReadWordBuffer;
pub use serialize::wordbuffer::writer::WriteWordBuffer;
pub use serialize::writer::WriteBuffer;
pub use tick::manager::{TickConfig, TickManager};
pub use tick::message::{
    PingMessage, PongMessage, SyncMessage, TimeSyncPingMessage, TimeSyncPongMessage,
};
pub use tick::ping_store::{PingId, PingStore};
pub use tick::time::{TimeManager, WrappedTime};
pub use tick::TickBufferChannel;
pub use transport::conditioner::LinkConditionerConfig;
pub use transport::io::{Io, IoConfig, TransportConfig};
pub use transport::udp::UdpSocket;
pub use utils::named::{Named, TypeNamed};
pub use utils::ready_buffer::ReadyBuffer;
pub use utils::sequence_buffer::SequenceBuffer;

pub mod prelude {
    pub use netcode::ClientId;

    pub use lightyear_derive::{component_protocol, message_protocol, Channel, Message};

    pub use crate::channel::channel::{
        Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
        DefaultUnorderedUnreliableChannel, EntityActionsChannel, EntityUpdatesChannel,
        InputChannel, PingChannel, ReliableSettings,
    };
    pub use crate::protocolize;
}

pub mod channel;
pub mod client;
pub mod connection;
pub mod inputs;
pub mod netcode;
pub mod packet;
pub mod plugin;
pub mod protocol;
pub mod replication;
pub mod serialize;
pub mod server;
pub mod tick;
pub mod transport;
pub mod utils;
