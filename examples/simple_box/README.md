# Simple box

A simple example that shows how to use Lightyear to create a server-authoritative multiplayer game.

It also showcases how to enable client-side prediction and snapshot interpolation:
- For the client sending inputs: the pink cube is client-predicted (so inputs are used with no delay, and there is a rollback in case of mismatch with the server) and the red cube shows the received server state. (the server state arrives with some delay, and is a bit choppy since the replication rate is only 10Hz).
- For the other clients: the red cube still shows the server states arriving at 10Hz, and the pink cube is a smooth interpolation between those states (there is a slight delay because we can only interpolate between 2 received server states).

https://github.com/cBournhonesque/lightyear/assets/8112632/7b57d48a-d8b0-4cdd-a16f-f991a394c852

## Running the example

There are different 'modes' of operation:

- as a dedicated server with `cargo run -- server`
- as a listen server with `cargo run -- client-and-server`. This will launch 2 independent bevy apps (client and server) in
  separate threads.
  They will communicate via channels (so with almost 0 latency)
- as a listen server with `cargo run -- host-server`. This will launch a single bevy app, where the server will also act
  as a client. Functionally, it is similar to the "client-and-server" mode, but you have a single bevy `World` instead of
  separate client and server `Worlds`s.

Then you can launch clients with the commands:

- `cargo run -- client -c 1` (`-c 1` overrides the client id, to use client id 1)
- `cargo run -- client -c 2`

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
