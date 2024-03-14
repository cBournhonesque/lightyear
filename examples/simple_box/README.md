# Simple box

A simple example that shows how to use Lightyear to create a server-authoritative multiplayer game.

It also showcases how to enable client-side prediction and snapshot interpolation.

https://github.com/cBournhonesque/lightyear/assets/8112632/7b57d48a-d8b0-4cdd-a16f-f991a394c852

## Running the example

You can either run the example as a "Listen Server" (the program acts as both client and server)
with: `cargo run -- listen-server`
or as dedicated server with `cargo run -- server`

Then you can launch multiple clients with the commands:

- `cargo run -- client -c 1`
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