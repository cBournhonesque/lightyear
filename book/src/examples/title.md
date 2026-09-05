# Examples

This page lists the examples in the [examples](https://github.com/cBournhonesque/lightyear/tree/main/examples) folder, roughly easiest first. Run the server with `cargo run -- server` and a client with `cargo run -- client -c 1` (add `--headless=false` for a GUI).

### Easy

- [simple_setup](https://github.com/cBournhonesque/lightyear/tree/main/examples/simple_setup): minimal example, just the client and server plugins and a connection.
- [simple_box](https://github.com/cBournhonesque/lightyear/tree/main/examples/simple_box): the tutorial example. Client/server prediction and interpolation, plus an optional deterministic input-only P2P mode.
- [bevy_enhanced_inputs](https://github.com/cBournhonesque/lightyear/tree/main/examples/bevy_enhanced_inputs): integrating lightyear with the `bevy_enhanced_input` crate for input handling.

### Medium

- [delta_compression](https://github.com/cBournhonesque/lightyear/tree/main/examples/delta_compression): replicate a component by sending only the difference when it changes, instead of the full value.
- [network_visibility](https://github.com/cBournhonesque/lightyear/tree/main/examples/network_visibility): only replicate a subset of entities to each player (interest management with rooms).
- [replication_groups](https://github.com/cBournhonesque/lightyear/tree/main/examples/replication_groups): replicate entities that refer to other entities (a component containing an `Entity`), with entity mapping so the references stay valid on the client.
- [priority](https://github.com/cBournhonesque/lightyear/tree/main/examples/priority): bandwidth management. Cap the bytes per second on a link and let priorities decide which updates go first.

### Advanced

- [avian_2d](https://github.com/cBournhonesque/lightyear/tree/main/examples/avian_2d) / [avian_3d](https://github.com/cBournhonesque/lightyear/tree/main/examples/avian_3d): replicate an Avian physics simulation (2D and 3D).
- [fps](https://github.com/cBournhonesque/lightyear/tree/main/examples/fps): prespawn bullets directly on the predicted timeline, with lag compensation for collisions between predicted and interpolated entities.
- [auth](https://github.com/cBournhonesque/lightyear/tree/main/examples/auth): how a client gets a `ConnectToken` from a backend to connect to a server.
- [lobby](https://github.com/cBournhonesque/lightyear/tree/main/examples/lobby): change the network topology at runtime; any client can become the host instead of the dedicated server.
- [deterministic_replication](https://github.com/cBournhonesque/lightyear/tree/main/examples/deterministic_replication): lockstep-style deterministic simulation.

There are also two bigger demos in [demos](https://github.com/cBournhonesque/lightyear/tree/main/demos): `spaceships` and `projectiles`.
