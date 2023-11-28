/*!
Lightyear is a networking library for Bevy.
It is designed for server-authoritative multiplayer games; and aims to be both feature-complete and easy-to-use.

You can find more information in the book!
!*/
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

// re-exports (mostly used in the derive macro crate or for internal purposes)
#[doc(hidden)]
pub mod _reexport {
    pub use enum_as_inner::EnumAsInner;
    pub use enum_delegate;
    pub use enum_dispatch::enum_dispatch;
    pub use paste::paste;

    pub use crate::channel::builder::{
        EntityActionsChannel, EntityUpdatesChannel, InputChannel, PingChannel,
    };
    pub use crate::client::interpolation::ShouldBeInterpolated;
    pub use crate::client::prediction::ShouldBePredicted;
    pub use crate::inputs::input_buffer::InputMessage;
    pub use crate::protocol::component::{
        ComponentBehaviour, ComponentKindBehaviour, ComponentProtocol, ComponentProtocolKind,
        IntoKind,
    };
    pub use crate::protocol::message::{MessageBehaviour, MessageKind, MessageProtocol};
    pub use crate::protocol::{BitSerializable, EventContext};
    pub use crate::serialize::reader::ReadBuffer;
    pub use crate::serialize::wordbuffer::reader::ReadWordBuffer;
    pub use crate::serialize::wordbuffer::writer::WriteWordBuffer;
    pub use crate::serialize::writer::WriteBuffer;
    pub use crate::shared::replication::ReplicationSend;
    pub use crate::tick::manager::TickManager;
    pub use crate::tick::time::{TimeManager, WrappedTime};
    pub use crate::tick::TickBufferChannel;
    pub use crate::utils::ready_buffer::ReadyBuffer;
    pub use crate::utils::sequence_buffer::SequenceBuffer;
}

/// Prelude containing commonly used types
pub mod prelude {
    pub use lightyear_derive::{component_protocol, message_protocol, Channel, Message};

    pub use crate::channel::builder::{
        Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
        DefaultUnorderedUnreliableChannel, ReliableSettings,
    };
    pub use crate::inputs::UserInput;
    pub use crate::netcode::{generate_key, ClientId, Key};
    pub use crate::packet::message::Message;
    pub use crate::protocol::channel::{ChannelKind, ChannelRegistry};
    pub use crate::protocol::Protocol;
    pub use crate::protocolize;
    pub use crate::shared::config::{LogConfig, SharedConfig};
    pub use crate::shared::plugin::SharedPlugin;
    pub use crate::shared::replication::resources::ReplicationData;
    pub use crate::shared::replication::{NetworkTarget, Replicate};
    pub use crate::shared::sets::{FixedUpdateSet, MainSet, ReplicationSet};
    pub use crate::tick::manager::TickConfig;
    pub use crate::tick::Tick;
    pub use crate::tick::TickBufferChannel;
    pub use crate::transport::conditioner::LinkConditionerConfig;
    pub use crate::transport::io::{Io, IoConfig, TransportConfig};
    pub use crate::transport::udp::UdpSocket;
    pub use crate::utils::named::{Named, TypeNamed};

    pub mod client {
        pub use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
        pub use crate::client::config::ClientConfig;
        pub use crate::client::config::NetcodeConfig;
        pub use crate::client::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent, MessageEvent,
        };
        pub use crate::client::input::{InputConfig, InputSystemSet};
        pub use crate::client::interpolation::interpolation_history::ConfirmedHistory;
        pub use crate::client::interpolation::plugin::{InterpolationConfig, InterpolationDelay};
        pub use crate::client::interpolation::{InterpolateStatus, Interpolated, LerpMode};
        pub use crate::client::ping_manager::PingConfig;
        pub use crate::client::plugin::{ClientPlugin, PluginConfig};
        pub use crate::client::prediction::plugin::PredictionConfig;
        pub use crate::client::prediction::predicted_history::{ComponentState, PredictionHistory};
        pub use crate::client::prediction::{Predicted, PredictionCommandsExt};
        pub use crate::client::resource::{Authentication, Client};
        pub use crate::client::sync::SyncConfig;
    }
    pub mod server {
        pub use crate::server::config::NetcodeConfig;
        pub use crate::server::config::ServerConfig;
        pub use crate::server::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent, MessageEvent,
        };
        pub use crate::server::ping_manager::PingConfig;
        pub use crate::server::plugin::{PluginConfig, ServerPlugin};
        pub use crate::server::resource::Server;
    }
}

/// Channels are used to add reliability/ordering on top of the transport layer
pub mod channel;

/// Defines the Client bevy resource
pub mod client;

/// A connection is a wrapper that lets us send message and apply replication
pub mod connection;

/// Handling of player inputs
pub mod inputs;

/// Netcode.io protocol to establish a connection on top of an unreliable transport
pub mod netcode;

/// How to build a packet
pub mod packet;

/// The Protocol is used to define all the types that can be sent over the network
pub mod protocol;

/// Serialization and deserialization of types
pub mod serialize;

/// Defines the Server bevy resource
pub mod server;

/// Defines a bevy plugin that is shared between the server and the client
pub mod shared;

/// Handles updating the internal tick and time
pub mod tick;

/// Provides an abstraction over an unreliable transport
pub mod transport;

/// Extra utilities
pub mod utils;
