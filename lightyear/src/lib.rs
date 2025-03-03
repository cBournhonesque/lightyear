/*! # Lightyear

Lightyear is a networking library for Bevy.
It is designed for server-authoritative multiplayer games; and aims to be both feature-complete and easy-to-use.

You can find more information in the [book](https://cbournhonesque.github.io/lightyear/book/) or check out the [examples](https://github.com/cBournhonesque/lightyear/tree/main/examples)!

## Getting started

### Install the plugins

`lightyear` provides two plugins groups: [`ServerPlugins`](prelude::server::ServerPlugins) and [`ClientPlugins`](prelude::client::ClientPlugins) that will handle the networking for you.

```rust
use bevy::utils::Duration;
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
[`ReplicationTarget`]: prelude::server::ReplicationTarget
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

    #[cfg(feature = "leafwing")]
    pub use crate::inputs::leafwing::{input_message::InputMessage, LeafwingUserAction};
    #[cfg(feature = "leafwing")]
    pub use crate::shared::input::leafwing::LeafwingInputPlugin;
    pub use crate::{
        channel::builder::{
            Channel, ChannelBuilder, ChannelContainer, ChannelDirection, ChannelMode,
            ChannelSettings, InputChannel, ReliableSettings,
        },
        client::prediction::prespawn::PreSpawnedPlayerObject,
        connection::{
            id::ClientId,
            netcode::{generate_key, ConnectToken, Key},
        },
        inputs::native::UserAction,
        packet::{error::PacketError, message::Message},
        protocol::{
            channel::{AppChannelExt, ChannelKind, ChannelRegistry},
            component::{AppComponentExt, ComponentRegistry, Linear},
            message::{
                registry::{AppMessageExt, MessageRegistry},
                resource::AppResourceExt,
            },
            serialize::AppSerializeExt,
        },
        shared::{
            config::SharedConfig,
            identity::{AppIdentityExt, NetworkIdentity, NetworkIdentityState},
            input::{native::InputPlugin, InputConfig},
            message::MessageSend,
            ping::manager::PingConfig,
            plugin::SharedPlugin,
            replication::{
                authority::HasAuthority,
                components::{
                    DeltaCompression, DisabledComponents, NetworkRelevanceMode,
                    OverrideTargetComponent, PrePredicted, ReplicateHierarchy,
                    ReplicateOnceComponent, Replicated, Replicating, ReplicationGroup,
                    ShouldBePredicted, TargetEntity,
                },
                entity_map::RemoteEntityMap,
                hierarchy::ParentSync,
                network_target::NetworkTarget,
                plugin::{ReplicationConfig, SendUpdatesMode},
                resources::{
                    ReplicateResourceExt, ReplicateResourceMetadata, StopReplicateResourceExt,
                },
            },
            run_conditions::*,
            sets::{FixedUpdateSet, MainSet},
            tick_manager::{Tick, TickConfig, TickManager},
            time_manager::TimeManager,
        },
        transport::middleware::{
            compression::CompressionConfig, conditioner::LinkConditionerConfig,
        },
        utils::history_buffer::{HistoryBuffer, HistoryState},
    };

    mod rename {
        pub use crate::{
            client::{
                connection::ConnectionManager as ClientConnectionManager,
                events::{
                    ComponentInsertEvent as ClientComponentInsertEvent,
                    ComponentRemoveEvent as ClientComponentRemoveEvent,
                    ComponentUpdateEvent as ClientComponentUpdateEvent,
                    ConnectEvent as ClientConnectEvent, DisconnectEvent as ClientDisconnectEvent,
                    EntityDespawnEvent as ClientEntityDespawnEvent,
                    EntitySpawnEvent as ClientEntitySpawnEvent,
                },
                message::{
                    ReceiveMessage as ClientReceiveMessage, ReceiveMessage as FromServer,
                    SendMessage as ClientSendMessage, SendMessage as ToServer,
                },
                replication::send::Replicate as ClientReplicate,
            },
            server::{
                connection::ConnectionManager as ServerConnectionManager,
                events::{
                    ComponentInsertEvent as ServerComponentInsertEvent,
                    ComponentRemoveEvent as ServerComponentRemoveEvent,
                    ComponentUpdateEvent as ServerComponentUpdateEvent,
                    ConnectEvent as ServerConnectEvent, DisconnectEvent as ServerDisconnectEvent,
                    EntityDespawnEvent as ServerEntityDespawnEvent,
                    EntitySpawnEvent as ServerEntitySpawnEvent,
                },
                message::{
                    ReceiveMessage as ServerReceiveMessage, ReceiveMessage as FromClients,
                    SendMessage as ServerSendMessage, SendMessage as ToClients,
                },
                replication::send::Replicate as ServerReplicate,
            },
        };
    }
    pub use rename::*;

    pub mod client {
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::client::{SocketConfig, SteamConfig};
        pub use crate::{
            client::{
                components::{ComponentSyncMode, Confirmed, LerpFn, SyncComponent, SyncMetadata},
                config::{ClientConfig, NetcodeConfig, PacketConfig},
                connection::ConnectionManager,
                error::ClientError,
                events::{
                    ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
                    DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent,
                },
                interpolation::{
                    interpolation_history::ConfirmedHistory,
                    plugin::{InterpolationConfig, InterpolationDelay, InterpolationSet},
                    InterpolateStatus, Interpolated, VisualInterpolateStatus,
                    VisualInterpolationPlugin,
                },
                io::{config::ClientTransport, Io},
                message::ReceiveMessage,
                networking::{ClientCommandsExt, ConnectedState, NetworkingState},
                plugin::ClientPlugins,
                prediction::{
                    correction::Correction,
                    despawn::PredictionDespawnCommandsExt,
                    plugin::{is_in_rollback, PredictionConfig, PredictionSet},
                    rollback::{Rollback, RollbackState},
                    Predicted,
                },
                replication::{
                    commands::DespawnReplicationCommandExt,
                    send::{Replicate, ReplicateToServer},
                },
                run_conditions::{is_connected, is_disconnected, is_synced},
                sync::SyncConfig,
            },
            connection::client::{
                Authentication, ClientConnection, IoConfig, NetClient, NetConfig,
            },
            protocol::message::client::ClientTriggerExt,
        };
    }
    pub mod server {
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        pub use wtransport::tls::Identity;

        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        pub use crate::connection::steam::server::{SocketConfig, SteamConfig};
        pub use crate::{
            connection::server::{IoConfig, NetConfig, NetServer, ServerConnection},
            protocol::message::server::ServerTriggerExt,
            server::{
                clients::ControlledEntities,
                config::{NetcodeConfig, PacketConfig, ServerConfig},
                connection::ConnectionManager,
                error::ServerError,
                events::{
                    ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, ConnectEvent,
                    DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent, InputEvent,
                },
                io::{config::ServerTransport, Io},
                networking::{NetworkingState, ServerCommandsExt},
                plugin::ServerPlugins,
                relevance::{
                    immediate::RelevanceManager,
                    room::{RoomId, RoomManager},
                },
                replication::{
                    commands::{AuthorityCommandExt, DespawnReplicationCommandExt},
                    send::{
                        ControlledBy, Lifetime, Replicate, ReplicationTarget, ServerFilter,
                        SyncTarget,
                    },
                    ReplicationSet, ServerReplicationSet,
                },
                run_conditions::{is_started, is_stopped},
            },
            shared::replication::authority::AuthorityPeer,
        };
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
