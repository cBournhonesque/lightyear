# Protocol

## Overview

The Protocol module in this library is responsible for defining the communication protocol used to send messages between
the client and server.
The Protocol must be shared between client and server, so that the messages can be serialized and deserialized
correctly.

## Key Concepts

### Protocol Trait

A `Protocol` contains multiple sub-parts:

- `Input`: Defines the user inputs, which is an enum of all the inputs that the client can send to the server.
  Input handling can be added by adding the `InputPlugin` plugin: `app.add_plugins(InputPlugin::<I>::default());`

- `LeafwingInput`: (only if the feature `leafwing` is enabled) Defines the leafwing `ActionState` that the client can
  send to the server.
  Input handling can be added by adding the `LeafwingInputPlugin` plugin: `app.add_plugins(LeafwingInputPlugin::<I>::default());`
 
- `MessageRegistry`: Will hold metadata about the all the messages that can be sent over the network. Each message must be `Serializable + Deserializeable + Clone`.
 You can register a message with the command `app.add_message::<Message1>(ChannelDirection::Bidirectional);`
 
- `Components`: Defines the component protocol, which is an enum of all the components that can be replicated between
  the client and server. Each component must be `Serializable + Clone + Component`.
  You can register a component with:
  ```rust,noplayground
  app.register_component::<PlayerId>(ChannelDirection::ServerToClient)
    .add_prediction::<PlayerId>(ComponentSyncMode::Once)
    .add_interpolation::<PlayerId>(ComponentSyncMode::Once);
  ```
  (You can specify additional behaviour for the component, such as prediction or interpolation.)

- `Channels`: the protocol should also contain a list of channels to be used to send messages. A `Channel` defines
  guarantees about how the packets will be sent over the network: reliably? in-order? etc.
  You can register a channel with:
  ```rust,noplayground
  app.add_channel::<Channel1>(ChannelSettings {
      mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
      ..default()
  });
  ```