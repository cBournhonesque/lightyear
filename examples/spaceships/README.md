# Spaceships Demo

This example extends what the `xpbd_physics` demo offers, making all entities server authoritative and
predicted by clients. 

* Client spaceships are spawned upon connect, and despawned when a client disconnects.
* All player actions are replicated immediately, and may arrive before server update for that tick
* Early inputs result in predicted bullet spawning to reduce perceived lag
* Visual smoothing and error correction used to blend in any rollback mispredictions
* All entities are predicted (ie, in your local timeline, ahead of server) so collisions between dynamic bodies should be non-janky
* Player labels: `25±2ms [3]` means 25ms server reported ping, 2ms jitter, 3 ticks of future inputs available
* Number of rollbacks and other metrics shown via screen diagnostics plugin

## Running the example

- Run the server: `cargo run server`
- Run client with id 1: `cargo run client -c 1`
- Run client with id 2: `cargo run client -c 2` (etc.)

You can modify the file `assets/settings.ron` to modify some networking settings.

### Testing in wasm with webtransport

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `trunk serve`

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:

- `sh examples/generate.sh` (to generate the temporary SSL certificates, they are only valid for 2 weeks)
- `cargo run -- server` to start the server. The server will print out the certificate digest (something
  like `1fd28860bd2010067cee636a64bcbb492142295b297fd8c480e604b70ce4d644`)
- You then have to replace the certificate digest in the `assets/settings.ron` file with the one that the server printed
  out.
- then start the client wasm test with `trunk serve`