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
- `LeafwingInput`: (only if the feature `leafwing` is enabled) Defines the leafwing `ActionState` that the client can
  send to the server.
- `Message`: Defines the message protocol, which is an enum of all the messages that can be exchanged between the client
  and server. Each message must be `Serializable + Clone`.
- `Components`: Defines the component protocol, which is an enum of all the components that can be replicated between
  the client and server. Each component must be `Serializable + Clone + Component`.
- `Channels`: the protocol should also contain a list of channels to be used to send messages. A `Channel` defines
  guarantees
  about how the packets will be sent over the network: reliably? in-order? etc.

### Protocolize Macro

The `protocolize!` macro is used to define a protocol. It takes the following parameters:

- `Self`: The name of the protocol.
- `Message`: The type of the message protocol.
- `Component`: The type of the component protocol.
- `Input`: The type of the user input.
- `Crate`: The name of the shared crate.

## Usage

To define a protocol, you would use the `protocolize!` macro. Here's a basic example:

```rust
protocolize! {
    Self = MyProtocol,
    Message = MyMessage,
    Component = MyComponent,
    Input = MyInput,
    Crate = my_crate,
}