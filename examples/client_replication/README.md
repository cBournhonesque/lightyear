# Introduction

A simple example that shows how to use lightyear for client-replication (the entity is spawned on the client and
replicated to the server):

- with client-authority: the cursor is replicated to the server and to other clients. Any client updates are replicated
  to the server.
  If we want to replicate it to other clients, we just needs to add the `Replicate` component on the server's entity to
  replicate the cursor to other clients.

- spawning pre-predicted entities on the client: when pressing the `Space` key, a square is spawned on the client. That
  square is a 'pre-predicted' entity:
  it will get replicated to the server. The server can replicate it back to all clients.
  When the original client gets the square back, it will spawn a 'Confirmed' square on the client, and will recognize
  that the original square spawned was a prediction. From there on it's normal replication.

- pressing `M` will send a message from a client to other clients

- pressing `K` will delete the Predicted entity. You can use this to confirm various rollback edge-cases.

https://github.com/cBournhonesque/lightyear/assets/8112632/718bfa44-80b5-4d83-a360-aae076f81fc3

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