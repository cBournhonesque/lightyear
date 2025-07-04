# Lobby

A simple example that shows how you can dynamically update the networking configuration at runtime. Whenever the client or server is disconnected, you can update the Client or Server's `NetConfig` and the changes will take effect at the next connection attempt!

The example contains:
- a dedicated server that will maintain a resource `Lobbies` containing the list of lobbies. This resource is replicated to all clients
- clients that can connect to the server and join a specific lobby.
- Inside a lobby, a client can click on the `StartGame` button to start a game. There is an option to choose who the host of the game will be. It can either be the dedicated
server (in which case we use `Rooms` to replicate separately for each lobby) or the host can be one of the clients which will run in `HostServer` mode (the client app also has a server running in the same process).


https://github.com/cBournhonesque/lightyear/assets/8112632/4ef661e6-b2e3-4b99-b1e3-1984925d0ffe


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
