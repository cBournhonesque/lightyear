# Priority

A simple example that shows how you can specify which messages/channels/entities have priority over others.
In case the bandwidth quota is reached, lightyear will only send the messages with the highest priority, up to the
quota.

To not starve lower priority entities, their priority is accumulated over time, so that they can eventually be sent.

In this example, rows have static priority marker components. The server writes Replicon's `PriorityMap`
for each connected client: the center row is low priority, the next rows are medium priority, and the
outer rows are high priority. `PriorityMap` only affects component mutations, so initial spawns are still
sent immediately.

You can find more information in
the [book](https://github.com/cBournhonesque/lightyear/blob/main/book/src/concepts/advanced_replication/bandwidth_management.md)

https://github.com/cBournhonesque/lightyear/assets/8112632/0efcd974-b181-4910-9312-5307fbd45718

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
