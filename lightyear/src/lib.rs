/*! # Lightyear

Lightyear is a networking library for Bevy.
It provides a set of plugins that can be used to create multiplayer games.

It has been tested mostly in the server-client topology, but the API should be flexible
enough to support other topologies such as peer-to-peer.

You can find more information in the [book](https://cbournhonesque.github.io/lightyear/book/) or check out the [examples](https://github.com/cBournhonesque/lightyear/tree/main/examples)!


## Getting started

### Adding the Plugins

Similarly to Bevy, lightyear is composed of several sub crates that each provide a set of features.
A wrapper crate `lightyear` is provided for convenience, which contain two main PluginGroups: [`ClientPlugins`](client::ClientPlugins) and [`ServerPlugins`](server::ServerPlugins) that you can add
to your app.

```rust
use bevy_app::App;
use core::time::Duration;
use lightyear::prelude::*;

pub const FIXED_TIMESTEP_HZ: f64 = 60.0;

fn main() {
    let mut app = App::new();
    app.add_plugins(client::ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });
    app.add_plugins(server::ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });
}
```

It is also possible to use the subcrates directly:

**IO** (How to send bytes over the network)
- lightyear_link: provides a transport-agnostic `Link` component which is responsible for sending and receiving bytes over the network.
- lightyear_crossbeam: IO layer that uses crossbeam channels. Useful for testing or for local networking (by having a server process and a client process on the same machine).
- lightyear_udp / lightyear_webtransport: IO layers for the UDP protocol and WebTransport protocol respectively.
- lightyear_aeronet: provides an integration layer to use [aeronet] as the IO layer.

**Connection**
- lightyear_connection: this layer wraps the IO layer by providing a long-running `PeerId` identifier component that is used to identify a peer in the network.
Also provides the `Client` and `Server` components for client-server topologies.
- lightyear_netcode: a connection layer that uses the [netcode.io](https://github.com/mas-bandwidth/netcode/blob/main/STANDARD.md) standard for creating secure connections over an unreliable IO
such as UDP.
- lightyear_steam: a connection layer that uses the Steam networking API to both send the bytes and to provide a long-running identifier. This layer operates at both the IO and the connection level.

Currently it is not possible to use an IO layer without a connection layer.

**Messages**
- lightyear_transport: provides a `Transport` component that is provides several channels with different reliability/ordering guarantees when sending raw bytes. This crate also organizes the raw
bytes into messages that are assembled into packets.
- lightyear_messages: provides a `MessageManager` component responsible for handling the serialization of `Messages` (serializable structs) into raw bytes that can be sent over the network.

**Replication**
- lightyear_replication: provides utilities to replicate the state of the Bevy World between two peers.
- lightyear_sync: helps synchronize the timelines between two peers.
- lightyear_prediction: provides client-side prediction and rollback to help hide latency
- lightyear_interpolation: provides interpolation for replicated entities to smooth out the network updates received from the remote peer.
- lightyear_frame_interpolation: most of the game logic should run in the FixedMain schedule, but the rendering is done in the PostUpdate schedule. To avoid visual artifacts, we need some interpolation to interpolate the rendering between the FixedMain states.

**Inputs**
- lightyear_inputs: backend-agnostic general input queue plugin to network client inputs
- lightyear_inputs_native: provides support to use any user-defined struct as an input type
- lightyear_inputs_leafwing: provides support to network [leafwing_input_manager] inputs
- lightyear_inputs_bei: provides support to network [bevy_enhanced_input] inputs

**Extra**
- lightyear_avian: provides a plugin to help handle networking Avian components. This sets the correct system ordering, etc.

**Utilities**
- lightyear_core: core components used by all the other crates.
- lightyear_utils: useful datastructures used by the other crates
- lightyear_serde: provides tools to serialize and deserialize structs while minimizing allocations.


### Implement the Protocol

The [`Protocol`](protocol) is a shared configuration between the local and remote peers that defines which types will be sent over the network.

You will have to define your protocol in a shared module that is accessible to both the client and the server.

There are several steps:
- [Adding messages](prelude::MessageRegistry#adding-messages)
- [Adding components](prelude::ComponentRegistry#adding-components)
- [Adding channels](prelude::ChannelRegistry#adding-channels)
- [Adding leafwing inputs](lightyear_inputs_leafwing#adding-leafwing-inputs) or [Adding inputs](lightyear_inputs_native#adding-a-new-input-type)

NOTE: the protocol must currently be added AFTER the Client/Server Plugins, but BEFORE any `Client` or `Server` entity is spawned.


### Spawn your Link entity

In lightyear, your Client or Server will simply be an entity with the right sets of components.

The [`Link`](prelude::Link) component can be added on an entity to make it able to send and receive data over the network.

Usually you will add more components on that entity to customize its behavior:
- define its role using the [`Client`](prelude::Client) or [`Server`](prelude::Server) components. Most of the lightyear plugins currently
expect the [`Link`](prelude::Link) entity to have one of these components.
- make it able to receive/send replication data using the [`ReplicationSender`](prelude::ReplicationSender) or [`ReplicationReceiver`](prelude::ReplicationReceiver) components.
- add 

The `Link`



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
//!
//! ### Feature Flags
#![doc = document_features::document_features!()]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![allow(ambiguous_glob_reexports)]

#[cfg(feature = "client")]
mod client;

#[cfg(all(feature = "server", not(target_family = "wasm")))]
mod server;
mod shared;

#[cfg(feature = "replication")]
mod protocol;
#[cfg(target_family = "wasm")]
mod web;

pub mod core {
    pub use lightyear_core::*;
}

#[cfg(feature = "crossbeam")]
pub mod crossbeam {
    pub use lightyear_crossbeam::*;
}

pub mod link {
    pub use lightyear_link::*;
}

#[cfg(feature = "netcode")]
pub mod netcode {
    pub use lightyear_netcode::*;
}

#[cfg(feature = "interpolation")]
pub mod interpolation {
    pub use lightyear_interpolation::*;
}

#[cfg(feature = "prediction")]
pub mod prediction {
    pub use lightyear_prediction::*;
}

#[cfg(feature = "webtransport")]
pub mod webtransport {
    pub use lightyear_webtransport::*;
}

#[cfg(any(feature = "input_native", feature = "leafwing", feature = "input_bei"))]
pub mod input {
    pub use lightyear_inputs::*;
    #[cfg(feature = "input_native")]
    pub mod native {
        pub use lightyear_inputs_native::*;
    }

    #[cfg(feature = "input_bei")]
    pub mod bei {
        pub use lightyear_inputs_bei::*;
    }

    #[cfg(feature = "leafwing")]
    pub mod leafwing {
        pub use lightyear_inputs_leafwing::*;
    }
}

pub mod connection {
    pub use lightyear_connection::*;
}

pub mod utils {
    pub use lightyear_utils::*;
}

pub mod prelude {
    pub use aeronet_io::connection::{LocalAddr, PeerAddr};
    pub use lightyear_connection::prelude::*;
    pub use lightyear_core::prelude::*;
    pub use lightyear_link::prelude::*;
    pub use lightyear_messages::prelude::*;
    #[cfg(feature = "replication")]
    pub use lightyear_replication::prelude::*;
    pub use lightyear_sync::prelude::*;
    pub use lightyear_transport::prelude::*;

    #[cfg(all(not(target_family = "wasm"), feature = "udp"))]
    pub use lightyear_udp::prelude::*;

    #[allow(unused_imports)]
    #[cfg(feature = "webtransport")]
    pub use lightyear_webtransport::prelude::*;

    #[cfg(feature = "netcode")]
    pub use lightyear_netcode::prelude::*;

    // TODO: maybe put this in prelude::client?
    #[cfg(feature = "prediction")]
    pub use lightyear_prediction::prelude::*;

    #[cfg(feature = "interpolation")]
    pub use lightyear_interpolation::prelude::*;

    #[cfg(any(feature = "input_native", feature = "leafwing", feature = "input_bei"))]
    pub mod input {
        pub use lightyear_inputs::prelude::*;

        #[cfg(feature = "input_native")]
        pub mod native {
            pub use lightyear_inputs_native::prelude::*;
        }

        #[cfg(feature = "input_bei")]
        pub mod bei {
            pub use lightyear_inputs_bei::prelude::*;
        }
        #[cfg(feature = "input_bei")]
        pub use lightyear_inputs_bei::prelude::InputRegistryExt;

        #[cfg(feature = "leafwing")]
        pub mod leafwing {
            pub use lightyear_inputs_leafwing::prelude::*;
        }
    }

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::ClientPlugins;

        pub use lightyear_sync::prelude::client::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::client::*;
        #[cfg(feature = "webtransport")]
        pub use lightyear_webtransport::prelude::client::*;

        #[cfg(any(feature = "input_native", feature = "leafwing", feature = "input_bei"))]
        pub mod input {
            pub use lightyear_inputs::prelude::client::*;
        }
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::ServerPlugins;
        pub use lightyear_connection::prelude::server::*;
        pub use lightyear_link::prelude::server::*;

        #[cfg(all(not(target_family = "wasm"), feature = "udp", feature = "server"))]
        pub use lightyear_udp::prelude::server::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::server::*;
        #[cfg(feature = "webtransport")]
        pub use lightyear_webtransport::prelude::server::*;

        #[cfg(any(feature = "input_native", feature = "leafwing", feature = "input_bei"))]
        pub mod input {
            pub use lightyear_inputs::prelude::server::*;
        }
    }
}
