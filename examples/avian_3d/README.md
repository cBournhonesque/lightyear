# Avian 3D Character

This is an example of a server containing server-authoritative, physics-based, 3D characters simulated with `avian3d` and clients controlling those characters and predicting their movement.

https://github.com/user-attachments/assets/4e0eb373-2f46-4c48-8157-46e1e5085097

## Features

* The client will immediately try to connect to the server on start.
* The server will spawn a new character for each client that connects and give that client control over the character.
  * A character is a dynamic 3D capsule.
  * The client can control the character with `W/A/S/D/SPACE`.
  * Client inputs are converted into physical forces applied to the character.
  * All clients predict every character so player-player and player-block collisions are simulated locally.
  * Each capsule has a touching, fixed-offset child cube collider reconstructed locally from the character template.
* The server will spawn a dynamic block and a static floor on start.
  * All clients predict the block.
  * The floor is only replicated and not predicted because we do not expect it to move.


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
