# Simple box

A simple example that shows how to use Lightyear to create a server-authoritative multiplayer game.

It also showcases how to enable client-side prediction and snapshot interpolation:
- For the client sending inputs: the pink cube is client-predicted (so inputs are used with no delay, and there is a rollback in case of mismatch with the server) and the red cube shows the received server state. (the server state arrives with some delay, and is a bit choppy since the replication rate is only 10Hz).
- For the other clients: the red cube still shows the server states arriving at 10Hz, and the pink cube is a smooth interpolation between those states (there is a slight delay because we can only interpolate between 2 received server states).

https://github.com/cBournhonesque/lightyear/assets/8112632/7b57d48a-d8b0-4cdd-a16f-f991a394c852

## Running an example

- Run the server with a gui: `cargo run -- server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run a headless client without a gui: `cargo run --no-default-features --features=client,netcode,webtransport -- client -c 1`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,webtransport,netcode`.

For automated headless verification, you can set `LIGHTYEAR_SIMPLE_BOX_AUTOMOVE=right` on one client and
`LIGHTYEAR_SIMPLE_BOX_LOG_POSITIONS=1` on another client to confirm from logs that the interpolated remote player
keeps receiving `PlayerPosition` updates.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so `certificates/generate.sh` is not required for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cd "$(git rev-parse --show-toplevel)" && sh certificates/generate.sh` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
