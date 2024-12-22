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

- Run the server: `cargo run --features=server`
- Run client with id 1: `cargo run --features=client -- -c 1`
- Run client with id 2: `cargo run --features=client -- -c 2` (etc.)
- Run the client and server in two separate bevy Apps: `cargo run --features=server,client`
- Run the server with a gui: `cargo run --features=server,gui`
- Run the client and server in "HostServer" mode, where the server is also a client (there is only one App): `cargo run --features=server,client -- -m=host-server`

You can modify the file `assets/settings.ron` to modify some networking settings.

### Testing in wasm with webtransport

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `trunk serve --features=client`

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:

- `cd "$(git rev-parse --show-toplevel)" && sh examples/certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
- `cargo run -- server` to start the server. The server will print out the certificate digest (something
  like `1fd28860bd2010067cee636a64bcbb492142295b297fd8c480e604b70ce4d644`)
- You then have to replace the certificate digest in the `assets/settings.ron` file with the one that the server printed
  out.
- then start the client wasm test with `trunk serve --features=client`