# Avian 3D Character

This is an example of a server containing server-authoritative, physics-based, 3D characters simulated with `avian3d` and clients controlling those characters and predicting their movement.

## Features

* The client will immediately try to connect to the server on start.
* The server will spawn a new character for each client that connects and give that client control over the character.
  * A character is a dynamic 3D capsule.
  * The client can control the character with `W/A/S/D/SPACE`.
  * Client inputs are converted into physical forces applied to the character.
  * All clients will predict the position, rotation, and velocity of all characters.
* The serve will spawn some dynamic blocks and a static floor on start.
  * All clients will predict the position, rotation, and velocity of all blocks.
  * The floor is only replicated and not predicted because we do not expect it to move.

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
