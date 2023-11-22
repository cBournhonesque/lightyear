# Protocol

## Overview

The Protocol module in this library is responsible for defining the communication protocol used to send messages between the client and server.

## Key Concepts

### Protocol Trait

The `Protocol` trait is the main interface for defining a protocol. It has several associated types:

- `Input`: Defines the user input type.
- `Message`: Defines the message protocol.
- `Components`: Defines the component protocol.
- `ComponentKinds`: Defines the component protocol kind.

It also provides methods for adding a channel and getting the channel registry.

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