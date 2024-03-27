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

// re-exports (mostly used in the derive macro crate or for internal purposes)
#[doc(hidden)]
pub mod _reexport {
    pub use enum_delegate;
    pub use enum_dispatch::enum_dispatch;
    pub use paste::paste;

    pub use lightyear_macros::{
        component_protocol_internal, message_protocol_internal, ChannelInternal, MessageInternal,
    };

    pub use crate::channel::builder::TickBufferChannel;
    pub use crate::channel::builder::{
        EntityActionsChannel, EntityUpdatesChannel, InputChannel, PingChannel,
    };
    pub use crate::client::interpolation::{
        add_interpolation_systems, add_prepare_interpolation_systems,
    };
    pub use crate::client::interpolation::{LinearInterpolator, NullInterpolator};
    pub use crate::client::prediction::add_prediction_systems;
    pub use crate::client::prediction::correction::{InstantCorrector, InterpolatedCorrector};
    pub use crate::protocol::component::{
        ComponentBehaviour, ComponentKindBehaviour, ComponentProtocol, ComponentProtocolKind,
        FromType,
    };
    pub use crate::protocol::message::InputMessageKind;
    pub use crate::protocol::message::{MessageKind, MessageProtocol};
    pub use crate::protocol::{BitSerializable, EventContext};
    pub use crate::serialize::reader::ReadBuffer;
    pub use crate::serialize::wordbuffer::reader::ReadWordBuffer;
    pub use crate::serialize::wordbuffer::writer::WriteWordBuffer;
    pub use crate::serialize::writer::WriteBuffer;
    pub use crate::shared::events::components::{
        ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, MessageEvent,
    };
    pub use crate::shared::events::connection::{
        IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
        IterMessageEvent,
    };
    pub use crate::shared::events::systems::{
        push_component_insert_events, push_component_remove_events, push_component_update_events,
    };
    pub use crate::shared::replication::components::ShouldBeInterpolated;
    pub use crate::shared::replication::systems::add_per_component_replication_send_systems;
    pub use crate::shared::replication::ReplicationSend;
    pub use crate::shared::time_manager::WrappedTime;
    pub use crate::utils::ready_buffer::{ItemWithReadyKey, ReadyBuffer};
    pub use crate::utils::sequence_buffer::SequenceBuffer;
}

/// Prelude containing commonly used types
pub mod prelude {
    pub use lightyear_macros::{component_protocol, message_protocol, Channel, Message};

    pub use crate::channel::builder::TickBufferChannel;
    pub use crate::channel::builder::{
        Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
        DefaultUnorderedUnreliableChannel, ReliableSettings,
    };
    pub use crate::client::prediction::prespawn::PreSpawnedPlayerObject;
    pub use crate::connection::netcode::{generate_key, ClientId, Key};
    #[cfg(feature = "leafwing")]
    pub use crate::inputs::leafwing::LeafwingUserAction;
    pub use crate::inputs::native::UserAction;
    pub use crate::packet::message::Message;
    pub use crate::protocol::channel::{ChannelKind, ChannelRegistry};
    pub use crate::protocol::Protocol;
    pub use crate::protocolize;
    pub use crate::shared::config::SharedConfig;
    pub use crate::shared::ping::manager::PingConfig;
    pub use crate::shared::plugin::{NetworkIdentity, SharedPlugin};
    pub use crate::shared::replication::components::{
        NetworkTarget, ReplicationGroup, ReplicationMode, ShouldBePredicted,
    };
    pub use crate::shared::replication::entity_map::{LightyearMapEntities, RemoteEntityMap};
    pub use crate::shared::replication::hierarchy::ParentSync;
    pub use crate::shared::replication::metadata::ClientMetadata;
    pub use crate::shared::sets::{FixedUpdateSet, MainSet, ReplicationSet};
    pub use crate::shared::tick_manager::TickManager;
    pub use crate::shared::tick_manager::{Tick, TickConfig};
    pub use crate::shared::time_manager::TimeManager;
    pub use crate::transport::conditioner::LinkConditionerConfig;
    pub use crate::transport::io::{Io, IoConfig, TransportConfig};
    pub use crate::utils::named::Named;

    pub mod client {
        pub use crate::client::components::{
            ComponentSyncMode, Confirmed, LerpFn, SyncComponent, SyncMetadata,
        };
        pub use crate::client::config::{ClientConfig, NetcodeConfig, PacketConfig};
        pub use crate::client::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent, MessageEvent,
        };
        pub use crate::client::input::{InputConfig, InputSystemSet};
        #[cfg(feature = "leafwing")]
        pub use crate::client::input_leafwing::{
            LeafwingInputConfig, LeafwingInputPlugin, ToggleActions,
        };
        pub use crate::client::interpolation::interpolation_history::ConfirmedHistory;
        pub use crate::client::interpolation::plugin::{
            InterpolationConfig, InterpolationDelay, InterpolationSet,
        };
        pub use crate::client::interpolation::{
            InterpolateStatus, Interpolated, VisualInterpolateStatus, VisualInterpolationPlugin,
        };
        pub use crate::client::metadata::GlobalMetadata;
        pub use crate::client::plugin::{ClientPlugin, PluginConfig};
        pub use crate::client::prediction::correction::Correction;
        pub use crate::client::prediction::plugin::is_in_rollback;
        pub use crate::client::prediction::plugin::{PredictionConfig, PredictionSet};
        pub use crate::client::prediction::predicted_history::{ComponentState, PredictionHistory};
        pub use crate::client::prediction::{Predicted, PredictionDespawnCommandsExt};
        pub use crate::client::sync::SyncConfig;
        pub use crate::connection::client::{
            Authentication, ClientConnection, NetClient, NetConfig,
        };
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::client::SteamConfig;
    }
    pub mod server {
        pub use crate::server::config::{NetcodeConfig, PacketConfig, ServerConfig};
        pub use crate::server::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent, MessageEvent,
        };
        pub use crate::server::metadata::GlobalMetadata;
        pub use crate::server::plugin::{PluginConfig, ServerPlugin};
        pub use crate::server::room::{RoomId, RoomManager, RoomMut, RoomRef};

        pub use crate::connection::server::{
            NetConfig, NetServer, ServerConnection, ServerConnections,
        };
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::server::SteamConfig;
        #[cfg(feature = "leafwing")]
        pub use crate::server::input_leafwing::LeafwingInputPlugin;
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        pub use wtransport::tls::Certificate;
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
