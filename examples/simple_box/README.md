# Simple box

A simple example that shows how to use Lightyear to create a server-authoritative multiplayer game.

It also showcases how to enable client-side prediction and snapshot interpolation:
- For the client sending inputs: the pink cube is client-predicted (so inputs are used with no delay, and there is a rollback in case of mismatch with the server) and the red cube shows the received server state. (the server state arrives with some delay, and is a bit choppy since the replication rate is only 10Hz).
- For the other clients: the red cube still shows the server states arriving at 10Hz, and the pink cube is a smooth interpolation between those states (there is a slight delay because we can only interpolate between 2 received server states).

https://github.com/cBournhonesque/lightyear/assets/8112632/7b57d48a-d8b0-4cdd-a16f-f991a394c852

## Running the example

- Run the server with a gui: `cargo run server`
- Run client with id 1: `cargo run client -c 1`
- Run client with id 2: `cargo run client -c 2`
- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`
- Run the server without a gui: `cargo run --no-default-features --features=server`
- Run the client and server in "HostServer" mode, where the server is also a client (there is only one App): `cargo run host-server`

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server`.
You can modify the file `assets/settings.ron` to modify some networking settings.

### Testing in wasm with webtransport

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:
- `cd "$(git rev-parse --show-toplevel)" && sh examples/certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
- Start the server with: `cargo run -- server`
- Then start the wasm client wasm with ``RUSTFLAGS='--cfg getrandom_backend="wasm_js"' trunk serve --features=client``
