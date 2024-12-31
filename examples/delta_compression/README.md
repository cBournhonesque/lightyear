# Delta compression

An example that shows how to enable delta-compression when replicating components.

In this example, instead of sending the updated `PlayerPosition` as a Vec2 (8 bytes), we only send 
the delta between the previous and the new position (2 bytes). This is done by implementing the
`Diffable` trait for the `PlayerPosition` component.

Delta compression can also be useful if you have a component that contains a large amount of data
(for example a Vec of 1000 elements) and you only need to update a few elements.


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

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:
- `cd "$(git rev-parse --show-toplevel)" && sh examples/certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
- Start the server with: `cargo run -- server`
- Then start the wasm client wasm with `trunk serve --features=client`
