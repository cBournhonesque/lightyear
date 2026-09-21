# Examples

This folder contains various examples showcasing `lightyear` features.

## Easy

Start here:
- `simple_setup`: minimal example that shows how to create a client and server and replicate an entity

Then:
- `simple_box`: example that showcases client/server prediction and interpolation, plus an optional deterministic input-only P2P mode
- `bevy_enhanced_inputs`: example that shows how to integrate lightyear with the `bevy_enhanced_input` crate to handle inputs.

## Medium

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

- `avian_2d`: example that shows how to replicate a 2D physics simulation using Avian.
  We also use the `leafwing` feature for a better way to manage inputs.
- `avian_3d`: example that shows clients controlling server-authoritative 3D objects simulated using Avian.
- `fps`: example that shows how to spawn player-objects directly on the Predicted timeline, and how to use lag compensation to compute collisions between predicted and interpolated entities.
- `timeline_switch`: example where players pick up physics blocks. Nearby free blocks are predicted, blocks carried by another player stay interpolated, showing how to move entities between timelines at runtime with `TimelineSwitch`.
- `auth`: an example that shows how a client can get a `ConnectToken` to connect to a server
- `lobby`: an example that shows how the network topology can be changed at runtime.
  Every client can potentially act as a host for the game (instead of the dedicated server).

## Running an example

Run these commands from an example using the shared CLI, such as `examples/simple_box`
(from the repository root, add `-p simple_box` to `cargo run`).

- Run the server headlessly (the default): `cargo run -- server`
- Run the server with a GUI: `cargo run -- --headless=false server`
- Run client with id 1: `cargo run -- client -c 1`

- Run the client and server in separate Bevy Apps connected by Crossbeam channels: `cargo run -- separate -c 1`
- Build and run the server without GUI support: `cargo run --no-default-features --features=server,webtransport,netcode -- server`
- Run the client and server in `HostClient` mode, in the same App: `cargo run -- host-client`

Features vary by example; see its `Cargo.toml`. The `simple_box` defaults include client, server,
GUI, P2P, netcode, and WebTransport. The shared client/server CLI selects WebTransport;
`--transport` belongs to the `p2p` subcommand, not `client` or `server`. To select UDP,
WebSocket, or Steam for client/server helpers, configure `ClientTransports` / `ServerTransports`
in the application's setup instead.

To build all compatible examples, use `just build_examples features=client,server`. Add
`headless=true` to omit the GUI from client/P2P builds, or `headless=false` to include it explicitly.
Use `names=avian_3d` or a comma-separated list such as `names=avian_2d,avian_3d` to build only
those example/demo packages.

Larger interactive applications, including the projectiles networking test bed,
live in [`demos/`](../demos/README.md).

Client IDs must be nonzero. A host client gets its own nonzero local alias by default;
use `host-client -c ID` only when you want to choose it explicitly.
`HostClient` is an in-process connection backend, not a Crossbeam link. Zero is reserved
for `PeerId::SERVER`, not a player number or a way to recognize the host.

For API migrations, see [numeric routing aliases](../book/src/tutorial/build_client_server.md#numeric-routing-aliases)
and [native dialing and failover](../book/src/tutorial/build_client_server.md#native-dialing-and-failover).
The latter covers UDP, hostname-preserving WS/WT clients, Steam targets and session facts,
and addressless Crossbeam setup. Native bind/dial facts belong to the transport, not `PeerId`.
The `lobby` example preserves its application-owned `LobbyClientId` when replacing the lobby
connection with a game-host connection; the new backend entity may have a different routing alias.
See [gameplay identity and connection replacement](../book/src/tutorial/build_client_server.md#gameplay-identity-and-connection-replacement)
and [routing scope](../book/src/concepts/connection/multi_connection.md) before composing endpoints.

The shared client/server examples use development authentication settings: native WebTransport
passes an empty pin with dangerous configuration enabled, and native WebSocket disables certificate
validation. Do not copy those settings into production. Use a trusted certificate policy or a
trusted WT digest; [identity-bound tokens](../book/src/tutorial/build_client_server.md#identity-bound-netcode-tokens)
are a separate, opt-in authorization mechanism, not a replacement for native authentication.

### P2P transports

P2P examples accept `--transport udp`, `--transport websocket`, or `--transport webtransport`.
Each peer advertises its native endpoint separately from its opaque routing alias.
WebSocket uses plain local connections. WebTransport generates a certificate per peer
and prints its SHA-256 digest at startup. In `simple_box`, join a WebTransport seed with:

```sh
cargo run --no-default-features --features=p2p -- p2p --transport webtransport --port 6100
cargo run --no-default-features --features=p2p -- p2p --transport webtransport --port 6101 --peer 127.0.0.1:6100 --peer-certificate-digest <64-hex-digit-seed-digest>
```

Only the seed pin must be supplied on the CLI; lobby discovery carries native addresses and
certificate pins for other members. Copy the seed digest through a trusted channel.
A successful pin check authenticates possession of that certificate, not an advertised routing
alias, lobby membership, or player/account. Discovery data from an untrusted peer does not
authorize those claims; the application must establish that trust separately. Restarting the
seed generates a new certificate, so use its newly printed digest.

UDP has no native authentication, and plain local WebSocket is not a secure discovery channel.
These recipes do not assign peer IDs, rename conflicting peers, or provide automatic
failover. The existing [P2P admission path](../book/src/tutorial/build_client_server.md#p2p-discovery-admission)
exchanges self-ID announcements, rejects conflicting claims, and requires the shared-roster barrier before gameplay.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so you do not need to run the certificate generator for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cargo run -p generate_certificate` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
