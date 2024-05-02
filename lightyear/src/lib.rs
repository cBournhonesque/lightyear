/*! # Lightyear

Lightyear is a networking library for Bevy.
It is designed for server-authoritative multiplayer games; and aims to be both feature-complete and easy-to-use.

You can find more information in the [book](https://cbournhonesque.github.io/lightyear/book/) or check out the [examples](https://github.com/cBournhonesque/lightyear/tree/main/examples)!
*/
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(clippy::type_complexity)]
#![allow(rustdoc::private_intra_doc_links)]
// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

// re-exports
#[doc(hidden)]
pub(crate) mod _internal {
    pub use paste::paste;
}

/// Prelude containing commonly used types
pub mod prelude {
    pub use lightyear_macros::Channel;

    pub use crate::channel::builder::TickBufferChannel;
    pub use crate::channel::builder::{
        Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
        DefaultUnorderedUnreliableChannel, ReliableSettings,
    };
    pub use crate::client::prediction::prespawn::PreSpawnedPlayerObject;
    pub use crate::connection::id::ClientId;
    pub use crate::connection::netcode::{generate_key, Key};
    #[cfg(feature = "leafwing")]
    pub use crate::inputs::leafwing::LeafwingUserAction;
    pub use crate::inputs::native::UserAction;
    pub use crate::packet::message::Message;
    pub use crate::protocol::channel::{AppChannelExt, ChannelKind, ChannelRegistry};
    pub use crate::protocol::component::{AppComponentExt, ComponentRegistry, Linear};
    pub use crate::protocol::message::{AppMessageExt, MessageRegistry};
    pub use crate::shared::config::{Mode, SharedConfig};
    pub use crate::shared::input::InputPlugin;
    #[cfg(feature = "leafwing")]
    pub use crate::shared::input_leafwing::LeafwingInputPlugin;
    pub use crate::shared::ping::manager::PingConfig;
    pub use crate::shared::plugin::{NetworkIdentity, SharedPlugin};
    pub use crate::shared::replication::components::{
        NetworkTarget, PrePredicted, Replicate, Replicated, ReplicationGroup, ReplicationMode,
        ShouldBePredicted,
    };
    pub use crate::shared::replication::entity_map::RemoteEntityMap;
    pub use crate::shared::replication::hierarchy::ParentSync;
    pub use crate::shared::replication::resources::{
        ReplicateResource, ReplicateResourceExt, StopReplicateResourceExt,
    };
    pub use crate::shared::sets::{FixedUpdateSet, MainSet};
    pub use crate::shared::tick_manager::TickManager;
    pub use crate::shared::tick_manager::{Tick, TickConfig};
    pub use crate::shared::time_manager::TimeManager;
    pub use crate::transport::config::{IoConfig, TransportConfig};
    pub use crate::transport::io::Io;
    pub use crate::transport::middleware::compression::CompressionConfig;
    pub use crate::transport::middleware::conditioner::LinkConditionerConfig;

    pub mod client {
        pub use crate::client::components::{
            ComponentSyncMode, Confirmed, LerpFn, SyncComponent, SyncMetadata,
        };
        pub use crate::client::config::{ClientConfig, NetcodeConfig, PacketConfig};
        pub use crate::client::connection::ConnectionManager;
        pub use crate::client::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent, MessageEvent,
        };
        pub use crate::client::input::{InputConfig, InputManager, InputSystemSet};
        #[cfg(feature = "leafwing")]
        pub use crate::client::input_leafwing::{LeafwingInputConfig, ToggleActions};
        pub use crate::client::interpolation::interpolation_history::ConfirmedHistory;
        pub use crate::client::interpolation::plugin::{
            InterpolationConfig, InterpolationDelay, InterpolationSet,
        };
        pub use crate::client::interpolation::{
            InterpolateStatus, Interpolated, VisualInterpolateStatus, VisualInterpolationPlugin,
        };
        pub use crate::client::networking::{ClientCommands, NetworkingState};
        pub use crate::client::plugin::ClientPlugin;
        pub use crate::client::prediction::correction::Correction;
        pub use crate::client::prediction::despawn::PredictionDespawnCommandsExt;
        pub use crate::client::prediction::plugin::is_in_rollback;
        pub use crate::client::prediction::plugin::{PredictionConfig, PredictionSet};
        pub use crate::client::prediction::rollback::{Rollback, RollbackState};
        pub use crate::client::prediction::Predicted;
        pub use crate::client::replication::ReplicationConfig;
        pub use crate::client::sync::SyncConfig;
        pub use crate::connection::client::{
            Authentication, ClientConnection, NetClient, NetConfig,
        };
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::client::SteamConfig;
    }
    pub mod server {
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        pub use wtransport::tls::Certificate;

        pub use crate::connection::server::{
            NetConfig, NetServer, ServerConnection, ServerConnections,
        };
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::server::SteamConfig;
        pub use crate::server::config::{NetcodeConfig, PacketConfig, ServerConfig};
        pub use crate::server::connection::ConnectionManager;
        pub use crate::server::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent, MessageEvent,
        };
        pub use crate::server::networking::{NetworkingState, ServerCommands};
        pub use crate::server::plugin::ServerPlugin;
        pub use crate::server::replication::{
            ReplicationConfig, ServerFilter, ServerReplicationSet,
        };
        pub use crate::server::room::{RoomId, RoomManager, RoomMut, RoomRef};
    }
}

pub mod channel;

pub mod client;

pub mod connection;

pub mod inputs;
pub mod packet;

pub mod protocol;

pub mod serialize;

pub mod server;

pub mod shared;

#[cfg(test)]
pub(crate) mod tests;

/// Provides an abstraction over an unreliable transport
pub mod transport;
/// Extra utilities
pub mod utils;
