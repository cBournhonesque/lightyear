# FPS Example

## Features

This example showcases several features that can be useful for building a multiplayer FPS.

It uses `AvianReplicationMode::Position { sync_to_transform: false }`: Avian `Position` and
`Rotation` are the replicated, predicted simulation state, while the renderer adds the type-erased
`FrameInterpolate` marker to predicted entities. Frame interpolation and rollback correction
operate on the physics pose, then the Avian integration writes the visual result to `Transform`
before transform propagation.


#### Prespawning

When you have a player-controlled predicted entity (the predicted `Player` in this example),
it can be useful to be able to spawn objects (here `Bullets`) directly in the predicted timeline.

You can achieve this by having a system that runs both on the client and server and spawns the same entity. The entity should have the `PreSpawnedPlayerObject`. That entity will be spawned 
immediately on the client in the predicted timeline. When the server spawns the entity, it will try replicating it to the client. There will be a matching step where that server entity will try to 
match with a prespawned client entity (using the spawn `Tick` and the entity's `Archetype`). After the matching is done, the bullet becomes a normal `Predicted` entity.

#### Bullet hits

Handling bullet hits can be tricky because the client is in the Predicted timeline but they shoot at enemies that are in the Interpolated timeline (so a bit in the past). There's 2 ways to solve 
this: 
- add Prediction to the target. This is only possible if the enemy movements can be predicted (with extrapolation, or because they move in a deterministic manner). In that case the player and the 
  target are in the same timeline
- use Lag Compensation

Here are a couple resources on lag compensation:
- https://developer.valvesoftware.com/wiki/Lag_Compensation
- https://gabrielgambetta.com/lag-compensation.html
  
The idea is that the server will adjust the hit detection by taking into account the interpolation delay of the client to simulate the hit from the point of view of the client. This works by 
storing a history buffer of the past few positions of each enemy so that the hit-detection can rewind those enemies in the past to see if it was a hit.

This can easily be achieved in `lightyear` in combination with `avian` by using the `LagCompensationSpatialQuery`.

In this example, the green enemy on the left is interpolated on the client and hits are detected via lag compensation. The blue enemy on the right is predicted on the client and hits are detected normally.



https://github.com/user-attachments/assets/17bc985d-f700-439d-ba48-4c69fbfd7885



## Running an example

- Run the server with a GUI: `cargo run -- --headless=false server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

### P2P mode

The example can also run as a deterministic input-only game with no server. Start peer 0 with
`cargo run --no-default-features --features=p2p -- --headless=true p2p --peer-id 0 --player-count 2`
and peer 1 with the same command using `--peer-id 1`. This mode simulates both targets and hit
detection on every peer; the conventional mode remains the example of server-side lag
compensation.

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,webtransport,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so you do not need to run the certificate generator for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cargo run -p generate_certificate` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
