# Examples

This folder contains various examples that showcase various `lightyear` features.

The top level `Cargo.toml` workspace defines the deps that examples can use and pick features from.


## Easy

- `simple_setup`: minimal example that just shows how to create the lightyear client and server plugins
- `simple_box`: example that showcases how to send inputs from client to server, and how to add client-prediction and interpolation

## Medium

- `replication_groups`: example that shows how to replicate entities that refer to other entities
  (e.g. they have a component containing an `Entity` id). You need to use `ReplicationGroup` to ensure that the
  those entities are replicated in the same message
- `interest_management`: example that shows how to use interest management to only replicate a subset of entities
  to each player, via the `VisibilityManager` and the `RoomManager`
- `client_replication`: example that shows how to replicate entities from the client to the server. (i.e. the client has authority)
- `priority`: example that shows how to manage bandwidth by enabling priority accumulation. Messages will be sent in
  order of their priority.

## Advanced

- `avian_physics`: example that shows how to replicate a physics simulation using xpbd.
  We also use the `leafwing` feature for a better way to manage inputs.
- `avian_3d_character`: example that shows clients controlling server-authoritative 3D objects simulated using avian.
- `spaceships`: more advanced version of `avian_physics` with player movement based on forces, fully server authoritative, predicted bullet spawning. 
- `bullet_prespawn`: example that shows how to spawn player-objects on the Predicted timeline. This is useful
  to avoid having to wait a full round-trip before the object is spawned.
- `auth`: an example that shows how a client can get a `ConnectToken` to connect to a server
- `lobby`: an example that shows how the network topology can be changed at runtime.
  Every client can potentially act as a host for the game (instead of the dedicated server).

## Running an example

Each example runs in a similar way.

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

## NOTES

The common crate provides the initial UI setup along with a connect/disconnect button, and manages
the bevygap stuff if needed.

## Building for Edgegap

```bash
# building the game server container
docker build -t examples -f examples/Dockerfile.server --progress=plain --build-arg examples="simple_box spaceships" .

# and to run, specify the example name as an env:
docker run --rm -it -e EXAMPLE_NAME=simple_box examples
# or with a key and an extra SANs for self-signed cert:
 docker run --rm -it -e EXAMPLE_NAME=simple_box -e LIGHTYEAR_PRIVATE_KEY="1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1" -e SELF_SIGNED_SANS="example.com,10.1.2.3" examples
```