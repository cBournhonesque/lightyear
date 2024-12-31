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

## Running the example

- Run the server: `cargo run --features=server`
- Run client with id 1: `cargo run --features=client -- -c 1`
- Run client with id 2: `cargo run --features=client -- -c 2` (etc.)
- Run the client and server in two separate bevy Apps: `cargo run --features=server,client`
- Run the server with a gui: `cargo run --features=server,gui`
- Run the client and server in "HostServer" mode, where the server is also a client (there is only one App): `cargo run --features=server,client -- -m=host-server`

You can modify the file `assets/settings.ron` to modify some networking settings.

### Testing in wasm with webtransport

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:
- `cd "$(git rev-parse --show-toplevel)" && sh examples/certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
- Start the server with: `cargo run -- server`
- Then start the wasm client wasm with `trunk serve --features=client`
