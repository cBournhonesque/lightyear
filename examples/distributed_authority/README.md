# Distributed authority

This example showcases how to transfer authority over an entity to the server or to a client.
This can be useful if you're going for a 'thin server' approach where clients are simulating most of the world.

In this example, the ball is initially simulated on the server.
When a client gets close the ball, the server transfers the authority over the ball to the client.
This means that the client is now simulating the ball and sending replication updates to the server.


https://github.com/user-attachments/assets/ee987fce-7a0d-4e76-a010-bc35b71e24cf



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
