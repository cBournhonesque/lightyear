# Replication groups

This is an example that shows how to make Lightyear replicate multiple entities in a single message,
to make sure that they are always in a consistent state (i.e. that entities in a group are all replicated on the same
tick).

Without a replication group, it is possible that one entity is replicated with the server's tick 10, and another entity
is replicated with the server's tick 11. This is not a problem if the entities are independent, but if they depend on
each other (for example
for client prediction) it could cause issues.

This is especially useful if you have an entity that depends on another entity (e.g. a player and its weapon),
the weapon might have a component `Parent(owner: Entity)` which references the player entity.
In which case we **need** the player entity to be spawned before the weapon entity, otherwise `Parent` component
will reference an entity that does not exist.

https://github.com/cBournhonesque/lightyear/assets/8112632/e7625286-a167-4f50-aa52-9175cc168287

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
