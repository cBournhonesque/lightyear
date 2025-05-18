# Interest management

A simple example that shows how to use Lightyear to perform interest management.

Interest management is a technique to reduce the amount of data that is sent to each client:
we want to send only the data that is relevant to each client.

In this example, we are going to replicate entities that are within a certain distance of the client.

https://github.com/cBournhonesque/lightyear/assets/8112632/41a6d102-77a1-4a44-8974-1d208b4ef798

## Running the example

- Run the server with a gui: `cargo run server`
- Run client with id 1: `cargo run client -c 1`
- Run client with id 2: `cargo run client -c 2`
- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`
- Run the server without a gui: `cargo run --no-default-features --features=server`
- Run the client and server in "HostServer" mode, where the server is also a client (there is only one App): `cargo run host-server`

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server`.
You can modify the file `assets/settings.ron` to modify some networking settings.

### Testing in wasm with webtransport

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:
- `cd "$(git rev-parse --show-toplevel)" && sh examples/certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
- Start the server with: `cargo run -- server`
- Then start the wasm client wasm with ``RUSTFLAGS='--cfg getrandom_backend="wasm_js"' trunk serve --features=client``
