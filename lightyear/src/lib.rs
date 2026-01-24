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

```rust,ignore
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
- [`lightyear_link`]: provides a transport-agnostic `Link` component which is responsible for sending and receiving bytes over the network.
- [`lightyear_crossbeam`]: IO layer that uses crossbeam channels. Useful for testing or for local networking (by having a server process and a client process on the same machine).
- [`lightyear_udp`] / [`lightyear_webtransport`]: IO layers for the UDP protocol and WebTransport protocol respectively.

**Connection**
- [`lightyear_connection`]: this layer wraps the IO layer by providing a long-running `PeerId` identifier component that is used to identify a peer in the network.
Also provides the `Client` and `Server` components for client-server topologies.
- [`lightyear_netcode`]: a connection layer that uses the [netcode.io](https://github.com/mas-bandwidth/netcode/blob/main/STANDARD.md) standard for creating secure connections over an unreliable IO
such as UDP.
- [`lightyear_steam`]: a connection layer that uses the Steam networking API to both send the bytes and to provide a long-running identifier. This layer operates at both the IO and
the connection
level.

Currently it is not possible to use an IO layer without a connection layer.

**Messages**
- [`lightyear_transport`]: provides a [`Transport`](prelude::Transport) component that is provides several channels with different reliability/ordering guarantees when sending raw bytes. This crate
also organizes
the
raw
bytes into messages that are assembled into packets.
- [`lightyear_messages`]: provides a [`MessageManager`](prelude::MessageManager) component responsible for handling the serialization of `Messages` (serializable structs) into raw bytes that can be
sent over
the network.

**Replication**
- [`lightyear_replication`]: provides utilities to replicate the state of the Bevy World between two peers.
- [`lightyear_sync`]: helps synchronize the timelines between two peers.
- [`lightyear_prediction`]: provides client-side prediction and rollback to help hide latency
- [`lightyear_interpolation`]: provides interpolation for replicated entities to smooth out the network updates received from the remote peer.
- [`lightyear_frame_interpolation`]: most of the game logic should run in the FixedMain schedule, but the rendering is done in the PostUpdate schedule. To avoid visual artifacts, we need some
interpolation to interpolate the rendering between the FixedMain states.

**Inputs**
- [`lightyear_inputs`]: backend-agnostic general input queue plugin to network client inputs
- [`lightyear_inputs_native`]: provides support to use any user-defined struct as an input type
- [`lightyear_inputs_leafwing`]: provides support to network [leafwing_input_manager](https://github.com/Leafwing-Studios/leafwing-input-manager) inputs
- [`lightyear_inputs_bei`]: provides support to network [bevy_enhanced_input](https://github.com/projectharmonia/bevy_enhanced_input) inputs

**Extra**
- [`lightyear_avian2d`]/[`lightyear_avian3d`]: provides a plugin to help handle networking Avian components. This sets the correct system ordering, etc.


### Implement the Protocol

The `Protocol` is a shared configuration between the local and remote peers that defines which types will be sent over the network.

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
- add a [`MessageManager`](prelude::MessageManager) component to handle the serialization and deserialization of messages.
- etc.

The `Server` entity works a bit differently. It starts a server that listens for incoming connections. When a new client connects, a new entity is spawned with the [`LinkOf`](prelude::LinkOf)
component.
You can add a trigger to listen to this event and add the extra components to customize the behaviour of this connection.

```rust
# use bevy_ecs::prelude::*;
# use lightyear::prelude::*;
# use core::time::Duration;
fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        ReplicationSender::new(Duration::from_millis(100), SendUpdatesMode::SinceLastAck, false),
        Name::from("Client"),
    ));
}
```

## Using lightyear

### Linking

There is a set that reflects if the [`Link`](prelude::Link) is established. The link represents an IO connection to send bytes to a remote peer.
You can trigger [`LinkStart`](prelude::LinkStart) to start the link, and [`Unlink`](prelude::Unlink) to stop it.

The [`Unlinked`](prelude::Unlinked), [`Linking`](prelude::Linking), [`Linked`](prelude::Linked) components represent the current state of the link.

### Connections

A connection is a wrapper around a [`Link`](prelude::Link) that provides a long-running identifier for the peer.
You can use the [`PeerId`](prelude::PeerId) component to identify the remote peer that the link is connected to, and [`LocalId`](prelude::LocalId) to identify the local peer.

The lifecycle of a connection is controlled by several sets of components.

You can trigger [`Connect`](prelude::Connect) to start the connection, and [`Disconnect`](prelude::Disconnect) to stop it.

The [`Disconnected`](prelude::Disconnected), [`Connecting`](prelude::Connecting), [`Connected`](prelude::Connected) components represent the current state of the connection.

On the server, [`Start`](prelude::server::Start) and [`Stop`](prelude::server::Stop) components are used to control the server's listening state.
The [`Stopped`](prelude::server::Stopped), [`Starting`](prelude::server::Starting), [`Started`](prelude::server::Started) components represent the current state of the connection.

While a client is disconnected, you can update its configuration (`ReplicationSender`, `MessageManager`, etc.), it will be applied on the next connection attempt.


### Sending messages

The [`MessageSender`](prelude::MessageSender) component is used to send messages that you have defined in your protocol.

```rust
# use bevy_ecs::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct MyMessage;

struct MyChannel;

fn send_message(mut sender: Single<&mut MessageSender<MyMessage>>) {
    let _ = sender.send::<MyChannel>(MyMessage);
}
```

### Receiving messages

The [`MessageReceiver`](prelude::MessageReceiver) component is used to receive messages that you have defined in your protocol.

```rust
# use bevy_ecs::prelude::*;
# use lightyear::prelude::*;
# use serde::{Serialize, Deserialize};

# #[derive(Serialize, Deserialize)]
# struct MyMessage;

fn send_message(mut receivers: Query<&mut MessageReceiver<MyMessage>>) {
    for mut receiver in receivers.iter_mut() {
        let _ = receiver.receive().for_each(|message| {});
    }
}
```

### Starting replication

To replicate an entity from the local world to the remote world, you can just add the [`Replicate`](prelude::Replicate) component to the entity.

The marker component [`Replicating`](prelude::Replicating) indicates that the entity is getting replicated to a remote peer.
You can remove the [`Replicating`](prelude::Replicating) component to pause the replication. This will not despawn the entity on the remote world; it will simply stop sending replication updates.


### Reacting to replication events

On the receiver side, entities that are replicated from a remote peer will have the [`Replicated`](prelude::Replicated) marker component.

You can use to react to components being inserted via replication.
```rust
# use bevy_ecs::prelude::*;
# use lightyear::prelude::*;
# use lightyear::prelude::client::*;
# use serde::{Serialize, Deserialize};

# #[derive(Component, Serialize, Deserialize)]
# struct MyComponent;

fn component_inserted(query: Query<Entity, (With<Replicated>, Added<MyComponent>)>) {
    for entity in query.iter() {
        println!("MyComponent was inserted via replication on {entity:?}");
    }
}
```

[`Replicated`]: prelude::Replicated
[`Replicating`]: prelude::Replicating
[`lightyear_steam`]: lightyear_steam
 */
//!
//! ### Feature Flags
#![doc = document_features::document_features!()]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![allow(ambiguous_glob_reexports)]

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;

mod shared;

#[cfg(feature = "replication")]
mod protocol;

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

#[cfg(feature = "frame_interpolation")]
pub mod frame_interpolation {
    pub use lightyear_frame_interpolation::*;
}

#[cfg(feature = "metrics")]
pub mod metrics {
    pub use lightyear_metrics::*;
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

#[cfg(feature = "steam")]
pub mod steam {
    pub use lightyear_steam::*;
}

#[cfg(feature = "webtransport")]
pub mod webtransport {
    pub use lightyear_webtransport::*;
}

#[cfg(feature = "websocket")]
pub mod websocket {
    pub use lightyear_websocket::*;
}

#[cfg(feature = "avian2d")]
pub mod avian2d {
    pub use lightyear_avian2d::*;
}

#[cfg(feature = "avian3d")]
pub mod avian3d {
    pub use lightyear_avian3d::*;
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
    #[cfg(feature = "metrics")]
    pub use lightyear_metrics::prelude::*;
    #[cfg(feature = "replication")]
    pub use lightyear_replication::prelude::*;
    pub use lightyear_serde::prelude::*;
    pub use lightyear_sync::prelude::*;
    pub use lightyear_transport::prelude::*;

    #[cfg(all(not(target_family = "wasm"), feature = "udp"))]
    pub use lightyear_udp::prelude::*;

    #[allow(unused_imports)]
    #[cfg(feature = "webtransport")]
    pub use lightyear_webtransport::prelude::*;

    #[allow(unused_imports)]
    #[cfg(feature = "websocket")]
    pub use lightyear_websocket::prelude::*;

    #[cfg(feature = "steam")]
    pub use lightyear_steam::prelude::*;

    #[cfg(feature = "netcode")]
    pub use lightyear_netcode::prelude::*;

    // TODO: maybe put this in prelude::client?
    #[cfg(feature = "prediction")]
    pub use lightyear_prediction::prelude::*;

    #[cfg(feature = "interpolation")]
    pub use lightyear_interpolation::prelude::*;

    #[cfg(feature = "debug")]
    pub use lightyear_ui::prelude::*;

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

        pub use lightyear_connection::prelude::client::*;
        pub use lightyear_sync::prelude::client::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::client::*;
        #[cfg(feature = "raw_connection")]
        pub use lightyear_raw_connection::prelude::client::*;
        #[cfg(feature = "steam")]
        pub use lightyear_steam::prelude::client::*;
        #[cfg(feature = "websocket")]
        pub use lightyear_websocket::prelude::client::*;
        #[cfg(feature = "webtransport")]
        pub use lightyear_webtransport::prelude::client::*;

        #[cfg(any(feature = "input_native", feature = "leafwing", feature = "input_bei"))]
        pub mod input {
            pub use lightyear_inputs::prelude::client::*;
        }
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::ServerPlugins;
        pub use lightyear_connection::prelude::server::*;
        pub use lightyear_link::prelude::server::*;

        #[cfg(all(not(target_family = "wasm"), feature = "udp", feature = "server"))]
        pub use lightyear_udp::prelude::server::*;

        #[cfg(feature = "netcode")]
        pub use lightyear_netcode::prelude::server::*;
        #[cfg(feature = "raw_connection")]
        pub use lightyear_raw_connection::prelude::server::*;
        #[cfg(feature = "steam")]
        pub use lightyear_steam::prelude::server::*;
        #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
        pub use lightyear_websocket::prelude::server::*;
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        pub use lightyear_webtransport::prelude::server::*;

        #[cfg(any(feature = "input_native", feature = "leafwing", feature = "input_bei"))]
        pub mod input {
            pub use lightyear_inputs::prelude::server::*;
        }
    }
}
