# Distributed authority

This example showcases how to transfer authority over an entity to the server or to a client.
This can be useful if you're going for a 'thin server' approach where clients are simulating most of the world.

In this example, the ball is initially simulated on the server.
When a client gets close the ball, the server transfers the authority over the ball to the client.
This means that the client is now simulating the ball and sending replication updates to the server.


https://github.com/user-attachments/assets/ee987fce-7a0d-4e76-a010-bc35b71e24cf



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
