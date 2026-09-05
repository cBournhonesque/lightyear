# Protocol

## Overview

The Protocol module in this library is responsible for defining the communication protocol used to send messages between
the client and server.

## Key Concepts

It must be shared between client and server (usually a single `ProtocolPlugin` added to both apps), so that messages can be serialized and deserialized correctly.
And it must be added **after** the `ClientPlugins` or `ServerPlugins`.

A protocol is composed of:

- [Inputs](../advanced_replication/inputs.md): the client's input type, i.e. the different actions a user can perform (move, jump, shoot, etc).
  Input handling is added with one of the input plugins, for example `app.add_plugins(input::native::InputPlugin::<Inputs>::default());`
  (there are equivalents for leafwing inputs and bevy-enhanced-inputs).

- Messages: the messages exchanged between client and server.
  Any `Send + Sync + 'static` type works. You register one with:
  ```rust,noplayground
  app.register_message::<Message1>()
      .add_direction(NetworkDirection::ServerToClient);
  ```
  The direction is only used to automatically add `MessageReceiver<M>`/`MessageSender<M>` on your Client/Sender entities,
  but you can also add these components manually.

- [Components](./title.md): the components that can be replicated from one `World` to the other.
  You register a component with:
  ```rust,noplayground
  app.component::<PlayerId>()
      .replicate()
      .predict()
      .add_linear_interpolation();
  ```
  (You specify additional behaviour per component: prediction, interpolation, correction...)

- [Channels](../reliability/channels.md): the delivery guarantees used to send messages.
  You register one with:
  ```rust,noplayground
  app.add_channel::<Channel1>(ChannelSettings {
      mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
      ..default()
  })
  .add_direction(NetworkDirection::ServerToClient);
  ```