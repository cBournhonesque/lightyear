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
    pub use crate::protocol::BitSerializable;
    pub use crate::replication::ReplicationSend;
    pub use crate::serialize::reader::ReadBuffer;
    pub use crate::serialize::wordbuffer::reader::ReadWordBuffer;
    pub use crate::serialize::wordbuffer::writer::WriteWordBuffer;
    pub use crate::serialize::writer::WriteBuffer;
    pub use crate::tick::manager::TickManager;
    pub use crate::tick::time::{TimeManager, WrappedTime};
    pub use crate::tick::TickBufferChannel;
    pub use crate::utils::ready_buffer::ReadyBuffer;
    pub use crate::utils::sequence_buffer::SequenceBuffer;
}

pub mod prelude {
    pub use lightyear_derive::{component_protocol, message_protocol, Channel, Message};

    pub use crate::channel::builder::{
        Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
        DefaultUnorderedUnreliableChannel, ReliableSettings,
    };
    pub use crate::inputs::input_buffer::UserInput;
    pub use crate::netcode::{generate_key, ClientId, Key};
    pub use crate::packet::message::Message;
    pub use crate::protocol::channel::{ChannelKind, ChannelRegistry};
    pub use crate::protocol::Protocol;
    pub use crate::protocolize;
    pub use crate::replication::{NetworkTarget, Replicate};
    pub use crate::shared::config::{LogConfig, SharedConfig};
    pub use crate::shared::plugin::SharedPlugin;
    pub use crate::shared::sets::{FixedUpdateSet, MainSet, ReplicationSet};
    pub use crate::shared::ReplicationData;
    pub use crate::tick::manager::TickConfig;
    pub use crate::tick::Tick;
    pub use crate::tick::TickBufferChannel;
    pub use crate::transport::conditioner::LinkConditionerConfig;
    pub use crate::transport::io::{Io, IoConfig, TransportConfig};
    pub use crate::transport::udp::UdpSocket;
    pub use crate::utils::named::{Named, TypeNamed};

    pub mod client {
        pub use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
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
        pub use crate::client::prediction::plugin::PredictionConfig;
        pub use crate::client::prediction::predicted_history::{ComponentState, PredictionHistory};
        pub use crate::client::prediction::{Predicted, PredictionCommandsExt};
        pub use crate::client::sync::SyncConfig;
        pub use crate::client::{Authentication, Client, ClientConfig, ClientPlugin, PluginConfig};
    }
    pub mod server {
        pub use crate::server::config::NetcodeConfig;
        pub use crate::server::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent, MessageEvent,
        };
        pub use crate::server::ping_manager::PingConfig;
        pub use crate::server::{PluginConfig, Server, ServerConfig, ServerPlugin};
    }
}

pub mod channel;
pub mod client;
pub mod connection;
pub mod inputs;
pub mod netcode;
pub mod packet;
pub mod protocol;
pub mod replication;
pub mod serialize;
pub mod server;
pub mod shared;
pub mod tick;
pub mod transport;
pub mod utils;
