/*! # Lightyear

Lightyear is a networking library for Bevy.
It is designed for server-authoritative multiplayer games; and aims to be both feature-complete and easy-to-use.

You can find more information in the [book](https://cbournhonesque.github.io/lightyear/book/) or check out the [examples](https://github.com/cBournhonesque/lightyear/tree/main/examples)!

## Getting started

### Install the plugins

`lightyear` provides two plugins groups: [`ServerPlugins`](prelude::server::ServerPlugins) and [`ClientPlugins`](prelude::client::ClientPlugins) that will handle the networking for you.

```rust
use core::time::Duration;
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;

fn run_client_app() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(ClientPlugins::new(ClientConfig::default()))
        .run();
}

fn run_server_app() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(ServerPlugins::new(ServerConfig::default()))
        .run();
}
```
In general, you will have to modify some parts of the [`ClientConfig`](prelude::client::ClientConfig) and [`ServerConfig`](prelude::server::ServerConfig) to fit your game.
Mostly the [`SharedConfig`], which must be the same on both the client and the server, and the `NetConfig` which defines
how the client and server will communicate.

### Implement the protocol

The [`Protocol`](protocol) is the set of types that can be sent over the network.
You will have to define your protocol in a shared module that is accessible to both the client and the server,
since the protocol must be shared between them.

There are several steps:
- [Adding messages](prelude::MessageRegistry#adding-messages)
- [Adding components](prelude::ComponentRegistry#adding-components)
- [Adding channels](prelude::ChannelRegistry#adding-channels)
- [Adding leafwing inputs](client::input::leafwing#adding-leafwing-inputs) or [Adding inputs](client::input::native#adding-a-new-input-type)

## Using lightyear

Lightyear provides various commands and resources that can you can use to interact with the plugin.

### Connecting/Disconnecting

On the client, you can initiate the connection by using the [`connect_client`](prelude::client::ClientCommands::connect_client) Command.
You can also disconnect with the [`disconnect_client`](prelude::client::ClientCommands::disconnect_client) Command.

On the server, you can start listening for connections by using the [`start_server`](prelude::server::ServerCommandsExt::start_server) Command.
You can stop the server using the [`stop_server`](prelude::server::ServerCommandsExt::stop_server) Command.

While the client or server are disconnected, you can update the [`ClientConfig`](prelude::client::ClientConfig) and [`ServerConfig`](prelude::server::ServerConfig) resources,
and the new configuration will take effect on the next connection attempt.

### Sending messages

On both the [client](prelude::client::ConnectionManager) and the [server](prelude::server::ConnectionManager), you can send messages using the `ConnectionManager` resource.

```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;

#[derive(Serialize, Deserialize)]
struct MyMessage;

#[derive(Channel)]
struct MyChannel;

fn send_message(mut connection_manager: ResMut<ConnectionManager>) {
    let _ = connection_manager.send_message_to_target::<MyChannel, MyMessage>(&MyMessage, NetworkTarget::All);
}
```

### Receiving messages

All network events are sent as Bevy events.
The full list is available [here](client::events) for the client, and [here](server::events) for the server.

Since they are Bevy events, you can use the Bevy event system to react to them.
```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;

# #[derive(Serialize, Deserialize)]
# struct MyMessage;

fn receive_message(mut message_reader: EventReader<ServerReceiveMessage<MyMessage>>) {
    for message_event in message_reader.read() {
       // the message itself
       let message = message_event.message();
       // the client who sent the message
       let client = message_event.from;
    }
}
```

### Starting replication

To replicate an entity from the local world to the remote world, you can just add the [`Replicate`](prelude::server::Replicate) bundle to the entity.
The [`Replicate`](prelude::server::Replicate) bundle contains many components to customize how the entity is replicated.

The marker component [`Replicating`] indicates that the entity is getting replicated to a remote peer.
You can remove the [`Replicating`] component to pause the replication. This will not despawn the entity on the remote world; it will simply
stop sending replication updates.

In contrast, the [`ReplicationTarget`] component is used to indicate which clients you want to replicate this entity to.
If you update the target to exclude a given client, the entity will get despawned on that client.

On the receiver side, entities that are replicated from a remote peer will have the [`Replicated`] marker component.


### Reacting to replication events

Similarly to messages, you can react to replication events using Bevy's event system.
```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::client::*;

# #[derive(Component, Serialize, Deserialize)]
# struct MyComponent;

fn component_inserted(mut events: EventReader<ComponentInsertEvent<MyComponent>>) {
    for event in events.read() {
       // the entity on which the component was inserted
       let entity = event.entity();
    }
}
```

Lightyear also inserts the [`Replicated`] marker component on every entity that was spawned via replication,
so you can achieve the same result with:
```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::client::*;

# #[derive(Component, Serialize, Deserialize)]
# struct MyComponent;

fn component_inserted(query: Query<Entity, (With<Replicated>, Added<MyComponent>)>) {
    for entity in query.iter() {
        println!("MyComponent was inserted via replication on {entity:?}");
    }
}
```

[`Replicated`]: prelude::Replicated
[`ReplicationTarget`]: prelude::server::ReplicateToClient
[`Replicating`]: prelude::Replicating
[`SharedConfig`]: prelude::SharedConfig
 */
#![allow(clippy::missing_transmute_annotations)]
#![allow(unused_variables)]
#![allow(clippy::too_many_arguments)]
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
    pub use serde::{Deserialize, Serialize};

    pub use crate::channel::builder::{
        Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode, ChannelSettings,
        InputChannel, ReliableSettings,
    };
    pub use crate::client::prediction::prespawn::PreSpawnedPlayerObject;
    pub use crate::connection::id::ClientId;
    pub use crate::connection::netcode::{generate_key, ConnectToken, Key};
    #[cfg(feature = "leafwing")]
    pub use crate::inputs::leafwing::{input_message::InputMessage, LeafwingUserAction};
    pub use crate::inputs::native::UserAction;
    pub use crate::packet::error::PacketError;
    pub use crate::packet::message::Message;
    pub use crate::protocol::channel::{AppChannelExt, ChannelKind, ChannelRegistry};
    pub use crate::protocol::component::{AppComponentExt, ComponentRegistry, Linear};
    pub use crate::protocol::message::{
        registry::{AppMessageExt, MessageRegistry},
        resource::AppResourceExt,
    };
    pub use crate::protocol::serialize::AppSerializeExt;
    pub use crate::shared::config::SharedConfig;
    pub use crate::shared::identity::{AppIdentityExt, NetworkIdentity, NetworkIdentityState};
    #[cfg(feature = "leafwing")]
    pub use crate::shared::input::leafwing::LeafwingInputPlugin;
    pub use crate::shared::input::native::InputPlugin;
    pub use crate::shared::message::MessageSend;
    pub use crate::shared::ping::manager::PingConfig;
    pub use crate::shared::plugin::SharedPlugin;
    pub use crate::shared::replication::authority::HasAuthority;
    pub use crate::shared::replication::components::{
        DeltaCompression, DisableReplicateHierarchy, DisabledComponents, NetworkRelevanceMode,
        PrePredicted, ReplicateOnceComponent, Replicated, Replicating, ReplicationGroup,
        ReplicationMarker, ShouldBePredicted, TargetEntity,
    };
    pub use crate::shared::replication::entity_map::RemoteEntityMap;
    pub use crate::shared::replication::hierarchy::{ChildOfSync, RelationshipSync, ReplicateLike};
    pub use crate::shared::replication::network_target::NetworkTarget;
    pub use crate::shared::replication::plugin::ReplicationConfig;
    pub use crate::shared::replication::plugin::SendUpdatesMode;
    pub use crate::shared::replication::resources::{
        ReplicateResourceExt, ReplicateResourceMetadata, StopReplicateResourceExt,
    };
    pub use crate::shared::run_conditions::*;
    pub use crate::shared::sets::{FixedUpdateSet, MainSet};
    pub use crate::shared::tick_manager::TickManager;
    pub use crate::shared::tick_manager::{Tick, TickConfig};
    pub use crate::shared::time_manager::TimeManager;
    pub use crate::transport::middleware::compression::CompressionConfig;
    pub use crate::transport::middleware::conditioner::LinkConditionerConfig;
    pub use crate::utils::history_buffer::{HistoryBuffer, HistoryState};

    mod rename {
        pub use crate::client::events::ComponentInsertEvent as ClientComponentInsertEvent;
        pub use crate::client::events::ComponentRemoveEvent as ClientComponentRemoveEvent;
        pub use crate::client::events::ComponentUpdateEvent as ClientComponentUpdateEvent;
        pub use crate::client::events::ConnectEvent as ClientConnectEvent;
        pub use crate::client::events::DisconnectEvent as ClientDisconnectEvent;
        pub use crate::client::events::EntityDespawnEvent as ClientEntityDespawnEvent;
        pub use crate::client::events::EntitySpawnEvent as ClientEntitySpawnEvent;
        pub use crate::client::message::ReceiveMessage as ClientReceiveMessage;
        pub use crate::client::message::ReceiveMessage as FromServer;
        pub use crate::client::message::SendMessage as ClientSendMessage;
        pub use crate::client::message::SendMessage as ToServer;
        pub use crate::client::replication::send::Replicate as ClientReplicate;

        pub use crate::client::connection::ConnectionManager as ClientConnectionManager;

        pub use crate::server::events::ComponentInsertEvent as ServerComponentInsertEvent;
        pub use crate::server::events::ComponentRemoveEvent as ServerComponentRemoveEvent;
        pub use crate::server::events::ComponentUpdateEvent as ServerComponentUpdateEvent;
        pub use crate::server::events::ConnectEvent as ServerConnectEvent;
        pub use crate::server::events::DisconnectEvent as ServerDisconnectEvent;
        pub use crate::server::events::EntityDespawnEvent as ServerEntityDespawnEvent;
        pub use crate::server::events::EntitySpawnEvent as ServerEntitySpawnEvent;
        pub use crate::server::message::ReceiveMessage as ServerReceiveMessage;
        pub use crate::server::message::ReceiveMessage as FromClients;
        pub use crate::server::message::SendMessage as ServerSendMessage;
        pub use crate::server::message::SendMessage as ToClients;

        pub use crate::server::connection::ConnectionManager as ServerConnectionManager;

        pub use crate::server::replication::send::Replicate as ServerReplicate;
    }
    pub use rename::*;

    pub mod client {
        pub use crate::client::components::{
            ComponentSyncMode, Confirmed, LerpFn, SyncComponent, SyncMetadata,
        };
        pub use crate::client::config::{ClientConfig, NetcodeConfig, PacketConfig};
        pub use crate::client::connection::ConnectionManager;
        pub use crate::client::error::ClientError;
        pub use crate::client::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent,
        };
        #[cfg(feature = "leafwing")]
        pub use crate::client::input::leafwing::LeafwingInputConfig;
        pub use crate::client::input::native::{InputConfig, InputManager};
        pub use crate::client::interpolation::interpolation_history::ConfirmedHistory;
        pub use crate::client::interpolation::plugin::{
            InterpolationConfig, InterpolationDelay, InterpolationSet,
        };
        pub use crate::client::interpolation::{
            InterpolateStatus, Interpolated, VisualInterpolateStatus, VisualInterpolationPlugin,
        };
        pub use crate::client::io::config::ClientTransport;
        pub use crate::client::io::Io;
        pub use crate::client::message::ReceiveMessage;
        pub use crate::client::networking::{ClientCommandsExt, ConnectedState, NetworkingState};
        pub use crate::client::plugin::ClientPlugins;
        pub use crate::client::prediction::correction::Correction;
        pub use crate::client::prediction::despawn::PredictionDespawnCommandsExt;
        pub use crate::client::prediction::plugin::is_in_rollback;
        pub use crate::client::prediction::plugin::{PredictionConfig, PredictionSet};
        pub use crate::client::prediction::rollback::{Rollback, RollbackState};
        pub use crate::client::prediction::Predicted;
        pub use crate::client::replication::commands::DespawnReplicationCommandExt;
        pub use crate::client::replication::send::{Replicate, ReplicateToServer};
        pub use crate::client::run_conditions::{is_connected, is_disconnected, is_synced};
        pub use crate::client::sync::SyncConfig;
        pub use crate::connection::client::{
            Authentication, ClientConnection, IoConfig, NetClient, NetConfig,
        };
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::client::{SocketConfig, SteamConfig};
        pub use crate::protocol::message::client::ClientTriggerExt;
    }
    pub mod server {
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        pub use wtransport::tls::Identity;

        pub use crate::connection::server::{IoConfig, NetConfig, NetServer, ServerConnection};
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::server::{SocketConfig, SteamConfig};
        pub use crate::protocol::message::server::ServerTriggerExt;
        pub use crate::server::clients::ControlledEntities;
        pub use crate::server::config::{NetcodeConfig, PacketConfig, ServerConfig};
        pub use crate::server::connection::ConnectionManager;
        pub use crate::server::error::ServerError;
        pub use crate::server::events::{
            ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
            DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent,
        };
        pub use crate::server::io::config::ServerTransport;
        pub use crate::server::io::Io;
        pub use crate::server::networking::{NetworkingState, ServerCommandsExt};
        pub use crate::server::plugin::ServerPlugins;
        pub use crate::server::relevance::immediate::RelevanceManager;
        pub use crate::server::relevance::room::{RoomId, RoomManager};
        pub use crate::server::replication::commands::AuthorityCommandExt;
        pub use crate::server::replication::commands::DespawnReplicationCommandExt;
        pub use crate::server::replication::{
            send::{
                ControlledBy, Lifetime, OverrideTarget, Replicate, ReplicateToClient, ServerFilter,
                SyncTarget,
            },
            ReplicationSet, ServerReplicationSet,
        };
        pub use crate::server::run_conditions::{is_started, is_stopped};
        pub use crate::shared::replication::authority::AuthorityPeer;
    }

    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    pub use crate::connection::steam::steamworks_client::SteamworksClient;
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
