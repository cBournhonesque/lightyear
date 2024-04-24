# Lobby

A simple example that shows how you can dynamically update the networking configuration at runtime.

The example contains:
- a dedicated server that will maintain a resource `Lobbies` containing the list of lobbies. This resource is replicated to all clients
- clients that can connect to the server and join a specific lobby.
- Inside a lobby, a client can click on the `StartGame` button to start a game. There is an option to choose who the host of the game will be. It can either be the dedicated
server (in which case we use `Rooms` to replicate separately for each lobby) or the host can be one of the clients which will run in `HostServer` mode (the client app also has a server running in the same process).


## Running the example

There are different 'modes' of operation:

- as a dedicated server with `cargo run -- server`

Then you can launch clients with the commands:

- `cargo run -- client -c 1` (`-c 1` overrides the client id, to use client id 1)
- `cargo run -- client -c 2`

You can modify the file `assets/settings.ron` to modify some networking settings.