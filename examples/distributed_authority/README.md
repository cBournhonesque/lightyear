# Distributed authority

This example showcases how to transfer authority over an entity to the server or to a client.
This can be useful if you're going for a 'thin server' approach where clients are simulating most of the world.

In this example, the ball is initially simulated on the server.
When a client gets close the ball, the server transfers the authority over the ball to the client.
This means that the client is now simulating the ball and sending replication updates to the server.


https://github.com/user-attachments/assets/ee987fce-7a0d-4e76-a010-bc35b71e24cf



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
