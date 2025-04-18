# General architecture

`Lightyear` essentially provides 2 plugins that will handle every networking-related concern for you: a `ClientPlugin`
and a `ServerPlugin`.

The plugins will define various resources and systems that will handle the connection to the server.
Some of the notable resources are:

- the [TickManager](https://docs.rs/lightyear/latest/lightyear/shared/tick_manager/struct.TickManager.html): `lightyear`
  uses `Ticks` to handle synchronization between the client and server. The `Tick` is basically the fixed-timestep unit
  of simulation, it gets incremented by 1 every time the FixedUpdate
  schedule runs. The `TickManager` has
  the [tick](https://docs.rs/lightyear/latest/lightyear/shared/tick_manager/struct.TickManager.html#method.tick) method
  to return the current client or server tick. (depending on which plugin you are using)
- the [ClientConnectionManager](https://docs.rs/lightyear/latest/lightyear/client/connection/struct.ConnectionManager.html)
or [ServerConnectionManager](https://docs.rs/lightyear/latest/lightyear/server/connection/struct.ConnectionManager.html)
which are used to send messages to the remote.
- the [ClientConnection](https://docs.rs/lightyear/latest/lightyear/prelude/client/struct.ClientConnection.html) or
  [ServerConnection](https://docs.rs/lightyear/latest/lightyear/prelude/server/struct.ServerConnection.html) which
  handle the general io connection. You can use them to get the current `ClientId` or to check that the connection is
  still alive.
- the [InputManager](https://docs.rs/lightyear/latest/lightyear/client/input/struct.InputManager.html) lets you send inputs from the client to the server

There are many different sub-plugins but the most important things that `lightyear` handles for you are probably:
- the sending and receiving of messages.
- automatic replication of the World from the server to the client
- handling the inputs from the user.


# Example code organization

In the most basic setup, you will run 2 separate apps: one for the client and one for the server.
(You can also run both in the same app in what is called `HostServer` mode, but we will not cover that in this
tutorial.)

The `simple_box` example has the following structure:

- `main.rs`: this is where we read the settings file from `assets/settings.ron` and create the client or server app
  depending on the passed CLI arguments.
- `settings.rs`: here we parse the `settings.ron` file and have helpers to create the `ClientConfig` and `ServerConfig`
  structs which are all that is required to build a `ClientPlugin` or a `ServerPlugin`
- `protocol.rs`: here we define a shared protocol, which is basically the list of messages, components and inputs that
  can be sent between the client and server.
- `shared.rs`: this is where we define shared behaviour between the client and server. For example some simulation
  logic (physics/movement) should be shared between the client and server.
- `client.rs`: this is where we define client-specific logic (input-handling, client-prediction, etc.)
- `server.rs`: this is where we define server-specific logic (spawning players for newly-connected clients, etc.)


## Defining a protocol

First, you will need to define a [protocol](../concepts/replication/protocol.md) for your game.
(see [here](https://github.com/cBournhonesque/lightyear/blob/main/examples/simple_box/src/protocol.rs) in the example)
This is where you define the "contract" of what is going to be sent across the network between your client and server.

A protocol is composed of

- [Input](../concepts/advanced_replication/inputs.md): Defines the client's input type, i.e. the different actions that
  a user can perform (e.g. move, jump, shoot, etc.).
- [Message](../concepts/bevy_integration/events.md): Defines the message protocol, i.e. the messages that can be
  exchanged between the client and server.
- [Components](../concepts/replication/title.md): Defines the component protocol, i.e. the list of components that can be replicated between the client and server.
- [Channels](../concepts/reliability/channels.md): Defines channels that are used to send messages between the client and server.

A `Message` is any struct that is `Serialize + Deserialize + Clone`.

### Components

The `ComponentRegistry` is needed for automatic World replication: automatically replicating entities and components
from the server's `World` to the client's `World`.
Only the components that are defined in the `ComponentRegistry` will be replicated.

The `ComponentRegistry` is a `Resource` that will store metadata about which components should be replicated and how.
It can also contain additional metadata for each component, such as prediction or interpolation settings.
`lightyear` provides helper functions on the `App` to register components to the `ComponentRegistry`.

Let's define our components protocol:

```rust
# use bevy::prelude::*;
# use lightyear::prelude::{client::ComponentSyncMode, *};
# use serde::{Deserialize, Serialize};
# use std::ops::{Add, Mul};
#
/// A component that will identify which player the box belongs to
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(ClientId);

/// A component that will store the position of the box. We could also directly use the `Transform` component.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerPosition(Vec2);

# impl Mul<f32> for &PlayerPosition {
#     type Output = PlayerPosition;
#
#     fn mul(self, rhs: f32) -> Self::Output {
#         PlayerPosition(self.0 * rhs)
#     }
# }
#
# impl Add for PlayerPosition {
#     type Output = PlayerPosition;
#     #[inline]
#     fn add(self, rhs: PlayerPosition) -> PlayerPosition {
#         PlayerPosition(self.0.add(rhs.0))
#     }
# }
#
/// A component that will store the color of the box, so that each player can have a different color.
#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin{
    fn build(&self, app: &mut App) {
        app.register_component::<PlayerId>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<PlayerPosition>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);
    }
}
```

Make sure `PlayerPosition` implements the `Linear` trait by implementing `Add` and `Mul` for it.

### Message

Similarly, the `MessageProtocol` is an enum containing the list of possible `Messages` that can be sent over the
network. When registering a message, you can specify the direction in which the message should be sent.

Let's define our message protocol:

```rust,noplayground
/// We don't really use messages in the example, but here is how you would define them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

/// Again, you need to use the macro `message_protocol` to define a message protocol.
#[message_protocol(protocol = "MyProtocol")]
pub enum Messages {
    Message1(Message1),
}

impl Plugin for ProtocolPlugin{
  fn build(&self, app: &mut App) {
    // Register messages
    app.add_message::<Message1>(ChannelDirection::Bidirectional);
    
    // Register components
    ...
  }
}
```

### Inputs

Lightyear handles inputs (the user actions that should be sent to the server) for you, you just need to define the list
of possible inputs (like the message or component protocols).

(it is recommended to use the `leafwing` feature to handle inputs with `leafwing-input-manager`, but we will not cover
that in this tutorial)

Let's define our inputs:

```rust
/// The different directions that the player can move the box
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Direction {
    pub(crate) up: bool,
    pub(crate) down: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}

/// The `InputProtocol` needs to be an enum of the various inputs that the client can send to the server.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum Inputs {
    Direction(Direction),
    Delete,
    Spawn,
    /// NOTE: we NEED to provide a None input so that the server can distinguish between lost input packets and 'None' inputs
    None,
}

impl Plugin for ProtocolPlugin{
  fn build(&self, app: &mut App) {
    // Register inputs
    app.add_plugins(InputPlugin::<Inputs>::default());
    // Register messages
    ...
    // Register components
    ...
  }
}
```

Inputs have to implement the `UserAction` trait, which means that they must be `Send + Sync + 'static` and can be serialized.

### Channels

We can also define some [channels](../concepts/reliability/channels.md) that will be used to send messages between the
client and server.
This is optional, since `lightyear` already provides some default channels for inputs and components.

A `Channel` defines some properties of how messages will be sent over the network:

- reliability: can the messages be lost or do we re-send them until we receive an ACK?
- ordering: do we guarantee that the messages are received in the same order that they were sent?
- priority: do we want to increase the priority of some messages in case the network is congested?

```rust,noplayground
/// A channel is basically a ZST (Zero Sized Type) with the `Channel` derive macro.
#[derive(Channel)]
pub struct Channel1;

pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
          mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
          ..default()
        });
        // register messages, inputs, components
        ...
    }
}
```

We create a channel by simply deriving the `Channel` trait on an empty struct.

### Protocol

The complete protocol looks like this:

```rust
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<Message1>(ChannelDirection::Bidirectional);
        // inputs
        app.add_plugins(InputPlugin::<Inputs>::default());
        // components
        app.register_component::<PlayerId>(ChannelDirection::ServerToClient)
            .add_prediction::<PlayerId>(ComponentSyncMode::Once)
            .add_interpolation::<PlayerId>(ComponentSyncMode::Once);

        app.register_component::<PlayerPosition>(ChannelDirection::ServerToClient)
            .add_prediction::<PlayerPosition>(ComponentSyncMode::Full)
            .add_interpolation::<PlayerPosition>(ComponentSyncMode::Full)
            .add_linear_interpolation_fn::<PlayerPosition>();

        app.register_component::<PlayerColor>(ChannelDirection::ServerToClient)
            .add_prediction::<PlayerColor>(ComponentSyncMode::Once)
            .add_interpolation::<PlayerColor>(ComponentSyncMode::Once);
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
```

## Summary

We now have a complete `Protocol` that defines:

- the data that can be sent between the client and server (inputs, messages, components)
- how the data will be sent (channels)

We can now start building our client and server Plugins.
