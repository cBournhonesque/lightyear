# Lightyear

A library for writing server-authoritative multiplayer games with [Bevy](https://bevyengine.org/).

Heavily inspired by [naia](https://github.com/naia-lib/naia).

https://github.com/cBournhonesque/lightyear/assets/8112632/def1fb1e-9f62-474d-8034-37aee300d54b

*Demo using one server with 2 clients. The entity is predicted (slightly ahead of server) on client 1, and interpolated (slightly behind server) on client 2.*

## Getting started

To quickly get started, you can follow this [tutorial](https://cbournhonesque.github.io/lightyear/book/tutorial/title.html), which re-creates the [simple_box](https://github.com/cBournhonesque/lightyear/tree/main/examples/simple_box) example.

You can also find more information in this WIP [book](https://cbournhonesque.github.io/lightyear/book/).

## Features

### Transport-agnostic

Lightyear uses a very general [Transport](https://github.com/cBournhonesque/lightyear/blob/main/lightyear/src/transport/mod.rs) trait to send raw data on the network.

The trait currently has two implementations: UDP sockets, and crossbeam channels (for testing).
I plan to add implementations for either WebRTC or WebTransport in the future, so that lightyear can be used to write browser games.

### Ergonomic

Lightyear provides a simple API for sending and receiving messages, and for replicating entities and components:
- the user needs to define a `Protocol` that defines all the messages, components, inputs that can be sent over the network; as well as the channels
- to send messages, the user mostly only needs to interact with the `Client<P>` and `Server<P>` structs, which provide methods to send messages and send inputs
- all messages are accessible via BevyEvents: `EventReader<MyMessage>` or `EventReader<EntitySpawnEvent>`
- for replication, the user just needs to add a `Replicate` component to entities that need to be replicated.

### Batteries-included

#### Serialization
Lightyear uses [bitcode](https://github.com/SoftbearStudios/bitcode/tree/main) for serialization, which supports very compact serialization.
It enables bit-packing (a bool will be serialized as a single bit).

#### Channels
Lightyear supports sending packets with different guarantees of ordering and reliability through the use of channels.

#### Input handling
Lightyear has special handling for player inputs (mouse presses, keyboards).
They are buffered every tick on the `Client`, and lightyear makes sure that the client input for tick `N` will be also processed on tick `N` on the server.
Inputs are protected against packet-loss: each packet will contain the client inputs for the last few frames.

#### Replication
Entities that have the `Replicate` component will be automatically replicated to clients. Only the components that change will be sent over the network.

This functionality is similar to what [bevy_replicon](https://github.com/lifescapegame/bevy_replicon) provides.

#### Advanced replication
Lightyear also supports easily enabling client-side prediction and snapshot interpolation to make sure that the game feels smooth for all players.
It is only a one-line change on the `Replicate` struct.

#### Configurable
Lightyear is highly configurable: you can configure the size of the input buffer, the amount of interpolation-delay, the packet send rate, etc.
All the configurations are accessible through the `ClientConfig` and `ServerConfig` structs.







On top of basic message-passing and replication, lightyear lets you easily add client-side prediction and snapshot interpolation to your game.


## Planned features

- Transport
    - [ ] Adding a web-compatible transport (WebRTC or WebTransport)
    - [ ] Add support for measuring packet loss
- Metrics
    - [ ] Improve the metrics/logs
- Packet
    - [ ] Add support for channel priority and channel bandwidth limiting
- Serialization
    - [ ] Improve the serialization code to be more ergonomic, and to have fewer copies.
- Replication 
    - [ ] Enable support for entity-relations: replicating components that contain references to other entities.
    - [ ] Add support for interest management: being able to only replicate entities to clients who are in the same 'zone' as them.
    - [ ] Add more tests for subtle replication situations (receiving ComponentInsert after the entity has been despawned, etc.)
