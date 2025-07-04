# Examples

This folder contains various examples that showcase various `lightyear` features.

The top level `Cargo.toml` workspace defines the deps that examples can use and pick features from.


## Easy

- `simple_setup`: minimal example that just shows how to create the lightyear client and server plugins
- `simple_box`: example that showcases how to send inputs from client to server, and how to add client-prediction and interpolation
- `bevy_enhanced_input`: example that shows how to integrate lightyear with the `bevy_enhanced_input` crate to handle inputs.

## Medium

- `client_replication`: example that shows how to replicate entities from the client to the server. (i.e. the client has authority)
- `delta_compression`: example that shows how a component can be replicated with delta-compression enabled. Whenever the component value
  changes, only the difference is sent over the network, instead of the full component value.
- `network_visibility`: example that shows how to use network visibility to only replicate a subset of entities
  to each player
- `replication_groups`: example that shows how to replicate entities that refer to other entities
  (e.g. they have a component containing an `Entity` id). You need to use `ReplicationGroup` to ensure that the
  those entities are replicated in the same message
- `priority`: example that shows how to manage bandwidth by enabling priority accumulation. Messages will be sent in
  order of their priority.

## Advanced

- `avian_physics`: example that shows how to replicate a physics simulation using xpbd.
  We also use the `leafwing` feature for a better way to manage inputs.
- `avian_3d_character`: example that shows clients controlling server-authoritative 3D objects simulated using avian.
- `spaceships`: more advanced version of `avian_physics` with player movement based on forces, fully server authoritative, predicted bullet spawning. 
- `fps`: example that shows how to spawn player-objects directly on the Predicted timeline, and how to use lag compensation to compute collisions between predicted and interpolated entities.
- `auth`: an example that shows how a client can get a `ConnectToken` to connect to a server
- `lobby`: an example that shows how the network topology can be changed at runtime.
  Every client can potentially act as a host for the game (instead of the dedicated server).

## Running an example

- Run the server with a gui: `cargo run -- server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,udp,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands to generate a self-signed certificate:
- `cd "$(git rev-parse --show-toplevel)" && sh certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)

