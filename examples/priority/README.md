# Priority

A simple example that shows how you can specify which messages/channels/entities have priority over others.
In case the bandwidth quota is reached, lightyear will only send the messages with the highest priority, up to the
quota.

To not starve lower priority entities, their priority is accumulated over time, so that they can eventually be sent.

In this example, the center row has priority 1.0, and each row further away from the center has a priority of +1.0.
(e.g. row 5 will get updated 5 times more frequently than row 1.0)

You can find more information in
the [book](https://github.com/cBournhonesque/lightyear/blob/main/book/src/concepts/advanced_replication/bandwidth_management.md)

https://github.com/cBournhonesque/lightyear/assets/8112632/0efcd974-b181-4910-9312-5307fbd45718

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