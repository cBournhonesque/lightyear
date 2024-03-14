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
