# Lightyear

[![crates.io](https://img.shields.io/crates/v/lightyear)](https://crates.io/crates/lightyear)
[![docs.rs](https://docs.rs/lightyear/badge.svg)](https://docs.rs/lightyear)
[![codecov](https://codecov.io/gh/cBournhonesque/lightyear/branch/main/graph/badge.svg?token=N1G28NQB1L)](https://codecov.io/gh/cBournhonesque/lightyear)

A library for writing server-authoritative multiplayer games with [Bevy](https://bevyengine.org/). Compatible with wasm
via WebTransport.

https://github.com/cBournhonesque/lightyear/assets/8112632/7b57d48a-d8b0-4cdd-a16f-f991a394c852

*Demo using one server with 2 clients. The entity is predicted (slightly ahead of server) on the controlling client and
interpolated (slightly behind server) on the other client.
The server only sends updates to clients 10 times per second but the clients still see smooth updates.*

## Getting started

You can first check out the [examples](https://github.com/cBournhonesque/lightyear/tree/main/examples).

To quickly get started, you can follow
this [tutorial](https://cbournhonesque.github.io/lightyear/book/tutorial/title.html), which re-creates
the [simple_box](https://github.com/cBournhonesque/lightyear/tree/main/examples/simple_box) example.

You can also find more information in this WIP [book](https://cbournhonesque.github.io/lightyear/book/).

## Related projects

- [lightyear-template](https://github.com/Piefayth/lightyear-template/tree/main): opiniated template for a bevy + lightyear starter project

### Games
- [Lumina](https://github.com/nixon-voxell/lumina)
- [cycles.io](https://github.com/cBournhonesque/jam5) for bevy jam 5: https://cbournhonesque.itch.io/cyclesio

## Features

### Ergonomic

*Lightyear* provides a simple API for sending and receiving messages, and for replicating entities and components:

- the user needs to define a shared protocol that defines all the `Messages`, `Components`, `Inputs` that can be sent over
  the network; as well as the `Channels` to be used:
```rust,ignore
// messages
app.add_message::<Message1>(ChannelDirection::Bidirectional);

// inputs
app.add_plugins(InputPlugin::<Inputs>::default());

// components
app.register_component::<PlayerId>(ChannelDirection::ServerToClient)
    .add_prediction(ComponentSyncMode::Once)
    .add_interpolation(ComponentSyncMode::Once);
    
// channels
app.add_channel::<Channel1>(ChannelSettings {
    mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
    ..default()
});
```
- to enable replication, the user just needs to add a `Replicate` bundle to entities that need to be replicated.
- all network-related events are accessible via bevy `Events`: `EventReader<MessageEvent<MyMessage>>` or `EventReader<EntitySpawnEvent>`
- I provide a certain number of bevy `Resources` to interact with the library (`InputManager`, `ConnectionManager`, `TickManager`,
  etc.)


### Batteries-included

- Transport-agnostic: *Lightyear* uses a very
  general [Transport](https://github.com/cBournhonesque/lightyear/blob/main/lightyear/src/transport/mod.rs) trait to
  send raw data on the network.
  The trait currently has several implementations:
    - UDP sockets
    - WebTransport (using QUIC): available on both native and wasm!
    - WebSocket: available on both native and wasm!
    - Steam: use the SteamWorks SDK to send messages over the Steam network
- Serialization
    - *Lightyear* uses `bincode` as a default serializer, but you can provide your own serialization function
- Message passing
    - *Lightyear* supports sending packets with different guarantees of ordering and reliability through the use of
      channels.
    - Packet fragmentation (for messages larger than ~1200 bytes) is supported
- Input handling
    - *Lightyear* has special handling for player inputs (mouse presses, keyboards).
      They are buffered every tick on the `Client`, and *lightyear* makes sure that the client input for tick `N` will
      be processed on tick `N` on the server.
      Inputs are protected against packet-loss: each packet will contain the client inputs for the last few frames.
    - With the `leafwing` feature, there is a special integration with
      the [`leafwing-input-manager`](https://github.com/Leafwing-Studios/leafwing-input-manager) crate, where
      your `leafwing` inputs are networked for you!
- World Replication
    - Entities that have the `Replicate` bundle will be automatically replicated to clients.
- Advanced replication
    - **Client-side prediction**: with just a one-line change, you can enable client-prediction with rollback on the
      client, so that your inputs can feel responsive
    - **Snapshot interpolation**: with just a one-line change, you can enable Snapshot interpolation so that entities
      are smoothly interpolated even if replicated infrequently.
    - **Client-authoritative replication**: you can also replicate entities from the client to the server. The authority over the entity is transferable between the client and the server.
    - **Pre-spawning predicted entities**: you can spawn Predicted entities on the client first, and then transfer the
      authority to the server. This ensures that the entity is spawned immediately, but will still be controlled by the server.
    - **Entity mapping**: *lightyear* also supports replicating components/messages that contain references to other
      entities. The entities will be mapped from the local World to the remote World.
    - **Interest management**: *lightyear* supports replicating only a subset of the World to clients. Interest
      management is made flexible by the use of `Rooms`
    - **Input Delay**: you can add a custom amount of input-delay as a trade-off between having a more responsive game
      or more mis-predictions
    - **Bandwidth Management**: you can set a cap to the bandwidth for the connection. Then messages will be sent in
      decreasing order of priority (that you can set yourself), with a priority-accumulation scheme
    - **Lag Compensation** is available so that predicted entities can interact with interpolated entities (used most often for fps games)
- Configurable
    - *Lightyear* is highly configurable: you can configure the size of the input buffer, the amount of
      interpolation-delay, the packet send rate, etc.
      All the configurations are accessible through the `ClientConfig` and `ServerConfig` structs.
- Observability
    - *Lightyear* uses the `tracing` and `metrics` libraries to emit spans and logs around most events (
      sending/receiving messages, etc.). The metrics can be exported to Prometheus for analysis.
- Examples
    - *Lightyear* has plenty of examples demonstrating all these features, as well as the integration with other bevy
      crates such as `avian`


## Supported bevy version

| Lightyear | Bevy |
|-----------|------|
| 0.20      | 0.16 |
| 0.18-0.19 | 0.15 |
| 0.16-0.17 | 0.14 |
| 0.10-0.15 | 0.13 |
| 0.1-0.9   | 0.12 |
