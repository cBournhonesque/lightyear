# General architecture

`lightyear` is split up into multiple crates that each provide a facet of networking.
The main crate `lightyear` provides an easy way of importing all the other crates and settings up the necessary plugins.
In particular it provides 2 plugin groups that set up the various systems needed for multiplayer app: [`ClientPlugins`](https://docs.rs/lightyear/latest/lightyear/prelude/client/struct.ClientPlugins.html) and [`ServerPlugins`](https://docs.rs/lightyear/latest/lightyear/prelude/client/struct.ServerPlugins.html).

There are many different sub-plugins that handle most of the complexities of networking, such as:

- Sending and receiving of messages
- Automatic replication of the World from the server to the client
- Syncing the timelines of the client and the server
- Handling the inputs from the user

# Example code organization

In a basic setup, you will run 2 separate apps: one for the client and one for the server.

The [`simple_box`](https://github.com/cBournhonesque/lightyear/tree/main/examples/simple_box) example has the following structure:

- `main.rs`: Creation of the client or server app depending on the passed CLI mode
- `protocol.rs`: Defines shared protocol, which is essentially the list of messages, components and inputs that
  can be sent between the client and server
- `shared.rs`: Defines shared behaviour between the client and server. For example,simulation logic like physics/movement should be shared between the client and server to ensure consistency.
- `client.rs`: Defines client-specific logic (input-handling, client-prediction, etc)
- `server.rs`: Defines server-specific logic (spawning players for newly-connected clients, etc)

## Adding the lightyear plugins

You will have to add the `ClientPlugins` and `ServerPlugins` to your app, depending on whether you are building a client or a server.

## Defining a protocol

After which, you will have to define a [protocol](../concepts/replication/protocol.md) for your game. The protocol must be added **after** the `ClientPlugins` or `ServerPlugins` are added to the app. (see [here](https://github.com/cBournhonesque/lightyear/blob/main/examples/simple_box/src/protocol.rs) in the example)

This is where you define the "contract" of what is going to be sent across the network between your client and server.

A protocol is composed of:

- [Input](../concepts/advanced_replication/inputs.md): Defines the client's input type, i.e. the different actions that a user can perform (e.g. move, jump, shoot, etc)
- [Message](../concepts/bevy_integration/events.md): Defines the message protocol, i.e. the messages that can be
  exchanged between the client and server
- [Components](../concepts/replication/title.md): Defines the component protocol, i.e. the list of components that can be replicated between the client and server
- [Channels](../concepts/reliability/channels.md): Defines channels that are used to send messages between the client and server

A `Message` is any struct that is `Serialize + Deserialize + Clone`.

### Components

The [`ComponentRegistry`](https://docs.rs/lightyear/latest/lightyear/prelude/client/struct.ComponentRegistry.html) is needed for automatic World replication: automatically replicating entities and components
from the server's `World` to the client's `World`.
Only the components that are defined in the `ComponentRegistry` will be replicated.

The `ComponentRegistry` is a `Resource` that will store metadata about which components should be replicated and how.
It can also contain additional metadata for each component, such as prediction or interpolation settings.
`lightyear` provides helper functions on the `App` to register components to the `ComponentRegistry`.

Let's define our component protocol:

```rust
/// A component that will identify which player the box belongs to
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(ClientId);

/// A component that will store the position of the box. We could also directly use the `Transform` component.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerPosition(Vec2);

/// A component that will store the color of the box, so that each player can have a different color.
#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin{
    fn build(&self, app: &mut App) {
        app.register_component::<PlayerId>();

        app.register_component::<PlayerPosition>();

        app.register_component::<PlayerColor>();
    }
}
```

### Message

Similarly, the [`MessageRegistry`](https://docs.rs/lightyear/latest/lightyear/prelude/client/struct.MessageRegistry.html) must contain the list of possible `Messages` that can be sent over the
network. When registering a message, you can specify the direction in which the message should be sent.

Let's define our message protocol:

```rust
/// We don't really use messages in the example, but here is how you would define them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

impl Plugin for ProtocolPlugin{
  fn build(&self, app: &mut App) {
    app.add_message::<Message1>()
      .add_direction(NetworkDirection::ServerToClient);
  }
}
```

### Inputs

As `lightyear` handles inputs, user actions that should be sent to the server; you have to define the list
of possible inputs (e.g message or component protocols)


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

#[derive(Serialize, Deserialize, Debug, PartialEq, Reflect, Eq, Clone)]
pub enum Inputs {
    Direction(Direction),
    Delete,
    Spawn,
}

// All inputs need to implement the `MapEntities` trait
impl MapEntities for Inputs {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

impl Plugin for ProtocolPlugin{
  fn build(&self, app: &mut App) {
    app.add_plugins(InputPlugin::<Inputs>::default());
  }
}
```

### Channels

We can also define some [channels](../concepts/reliability/channels.md) that will be used to send messages between the
client and server.
This is optional, since `lightyear` already provides some default channels for inputs and components.

A `Channel` defines some properties of how messages will be sent over the network:

- **Reliability**: Can the messages be lost or do we re-send them until we receive an ACK?
- **Ordering**: Do we guarantee that the messages are received in the same order that they were sent?
- **Priority**: Which messages to send in priority if we have reached the max bandwidth of the network?

```rust
pub struct Channel1;

pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<Channel1>(ChannelSettings {
          mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
          ..default()
        });
    }
}
```

We create a channel by simply deriving the `Channel` trait on an empty struct.

## Summary

We now have a complete `Protocol` that defines:

- Data that can be sent between the client and server (inputs, messages, components)
- How the data will be sent (channels)

We can now start building our client and server.
