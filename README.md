# Lightyear

[![crates.io](https://img.shields.io/crates/v/lightyear)](https://crates.io/crates/lightyear)
[![docs.rs](https://docs.rs/lightyear/badge.svg)](https://docs.rs/lightyear)
[![codecov](https://codecov.io/gh/cBournhonesque/lightyear/branch/main/graph/badge.svg?token=N1G28NQB1L)](https://codecov.io/gh/cBournhonesque/lightyear)

A library for writing server-authoritative multiplayer games with [Bevy](https://bevyengine.org/).

Heavily inspired by [naia](https://github.com/naia-lib/naia).


https://github.com/cBournhonesque/lightyear/assets/8112632/7b57d48a-d8b0-4cdd-a16f-f991a394c852

*Demo using one server with 2 clients. The entity is predicted (slightly ahead of server) on the controlling client and interpolated (slightly behind server) on the other client.
The server only sends updates to clients 10 times per second but the clients still see smooth updates.*



## Getting started

To quickly get started, you can follow this [tutorial](https://cbournhonesque.github.io/lightyear/book/tutorial/title.html), which re-creates the [simple_box](https://github.com/cBournhonesque/lightyear/tree/main/examples/simple_box) example.

You can also find more information in this WIP [book](https://cbournhonesque.github.io/lightyear/book/).

## Features

### Transport-agnostic

Lightyear uses a very general [Transport](https://github.com/cBournhonesque/lightyear/blob/main/lightyear/src/transport/mod.rs) trait to send raw data on the network.

The trait currently has two implementations:
- UDP sockets
- WebTransport (using QUIC): not compatible with wasm yet.

### Ergonomic

Lightyear provides a simple API for sending and receiving messages, and for replicating entities and components:
- the user needs to define a `Protocol` that defines all the messages, components, inputs that can be sent over the network; as well as the channels
- to send messages, the user mostly only needs to interact with the `Client<P>` and `Server<P>` structs, which provide methods to send messages and send inputs
- all messages are accessible via BevyEvents: `EventReader<MessageEvent<MyMessage>>` or `EventReader<EntitySpawnEvent>`
- for replication, the user just needs to add a `Replicate` component to entities that need to be replicated.

### Batteries-included

- Serialization
  - Lightyear uses [bitcode](https://github.com/SoftbearStudios/bitcode/tree/main) for serialization, which supports very compact serialization. It uses bit-packing (a bool will be serialized as a single bit).
- Reliability
  - Lightyear supports sending packets with different guarantees of ordering and reliability through the use of channels.
- Input handling
  - Lightyear has special handling for player inputs (mouse presses, keyboards).
    They are buffered every tick on the `Client`, and lightyear makes sure that the client input for tick `N` will be also processed on tick `N` on the server.
    Inputs are protected against packet-loss: each packet will contain the client inputs for the last few frames.
- Replication
  - Entities that have the `Replicate` component will be automatically replicated to clients. Only the components that change will be sent over the network. This functionality is similar to what [bevy_replicon](https://github.com/lifescapegame/bevy_replicon) provides.
- Advanced replication
  - Lightyear also supports easily enabling client-side prediction and snapshot interpolation to make sure that the game feels smooth for all players.
    It is only a one-line change on the `Replicate` struct.
  - Lightyear also supports replicating components that contain references to other entities. The entities will be mapped from the Server World to the Client World upon replication.
- Configurable
  - Lightyear is highly configurable: you can configure the size of the input buffer, the amount of interpolation-delay, the packet send rate, etc.
    All the configurations are accessible through the `ClientConfig` and `ServerConfig` structs.
- Observability
  - Lightyear uses the `tracing` and `metrics` libraries to emit spans and logs around most events (sending/receiving messages, etc.). The metrics
    can be exported to Prometheus for analysis.


## Planned features

- Metrics
    - Improve the metrics/logs
- Packet
    - Add support for channel priority and channel bandwidth limiting
- Serialization
    - Improve the serialization code to be more ergonomic, and to have fewer copies.
- Replication 
    - Add support for interest management: being able to only replicate entities to clients who are in the same 'zone' as them.
    - Add more tests for subtle replication situations (receiving ComponentInsert after the entity has been despawned, etc.)
