# Delta compression

An example that shows how to use Replicon's diff-based component replication.

In this example, `PlayerTrail` is the only replicated movement component. It is a
`Vec<TrailPoint>` containing the current player-controlled head point followed by older trail
points. Instead of sending the whole trail every time the player moves, the server records a
small Replicon `Diffable` update that inserts the newest head point.

Diff replication is useful when a component contains a larger collection and each update only changes
a small part of it. `PlayerTrail` also uses custom interpolation for diff-replicated components so
remote trails slide smoothly between materialized diff states.


## Running an example

- Run the server with a gui: `cargo run -- server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,webtransport,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so `certificates/generate.sh` is not required for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cd "$(git rev-parse --show-toplevel)" && sh certificates/generate.sh` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
