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

- `cd "$(git rev-parse --show-toplevel)" && sh examples/certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
- `cargo run -- server` to start the server. The server will print out the certificate digest (something
  like `1fd28860bd2010067cee636a64bcbb492142295b297fd8c480e604b70ce4d644`)
- You then have to replace the certificate digest in the `assets/settings.ron` file with the one that the server printed
  out.
- then start the client wasm test with `trunk serve`