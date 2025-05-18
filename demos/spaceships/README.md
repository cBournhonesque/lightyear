# Spaceships Demo

This example extends what the `xpbd_physics` demo offers, making all entities server authoritative and
predicted by clients. Bullets are prespawned when you fire, or remote players fire if we have their inputs early.

**Controls:** `Up/Left/Right/Space`

<img width="90%" alt="spaceships-screenshot" src="https://github.com/RJ/lightyear/assets/29747/698237c0-56fa-4dd8-a341-a49834d0e107">


* Client spaceships are spawned upon connect, and despawned when a client disconnects.
* All player actions are replicated immediately, and may arrive before server update for that tick
* Early inputs result in predicted bullet spawning to reduce perceived lag
* Visual smoothing and error correction used to blend in any rollback mispredictions
* All entities are predicted (ie, in your local timeline, ahead of server) so collisions between dynamic bodies should be non-janky
* Player labels: `25±2ms [3]` means 25ms server reported ping, 2ms jitter, 3 ticks of future inputs available
* Number of rollbacks and other metrics shown via screen diagnostics plugin


### Predicted bullet spawning and Input Delay

When you press fire, the bullet is prespawned with a `PreSpawnedPlayerObject` hash. The server spawns with the
matching hash, and once the `Confirmed` entity is replicated, your prespawned entity becomes the `Predicted` entity. See the [Prespawning chapter](https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/prespawning.html) of the Lightyear book for more information.

Notably, when players have an input delay configured (eg. on tick 10 you sample inputs for tick 13), 
since these inputs are immediately sent to the server, which broadcasts them to other players, it's
possible to receive remote players' inputs for a tick before you simulate that tick on the client.

In this scenario, the remote player's bullet will also be predictively spawned just like for the local player.

Should the remote player inputs not arrive before your client simulates the tick, 
the bullet will be created when the server spawns it and replicates through normal means. In this case
your client will rollback to position the bullet correctly.


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
