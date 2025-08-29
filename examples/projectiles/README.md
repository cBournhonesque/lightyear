# Projectiles Example

## Features

This example showcases several features that can be useful for building a multiplayer FPS, including different weapon types, projectile replication strategies, and networking approaches.

### Weapon Types

The example now supports multiple weapon types that you can cycle through:

1. **Hitscan** - Instant hit with fast visual effect
2. **Hitscan with Slow Visuals** - Instant hit but with slower visual trail
3. **Linear Projectile** - Simple projectile with constant velocity
4. **Shotgun** - Multiple pellets with spread
5. **Physics Projectile** - Projectile with physics interactions (bouncing, deceleration)
6. **Homing Missile** - Projectile that tracks nearest target

**Controls:**
- `Q` - Cycle through weapon types
- `Space` - Shoot current weapon

### Projectile Replication Methods

Three different approaches to replicating projectiles:

1. **Full Entity Replication** - Traditional approach where the entire projectile entity is replicated with regular updates. Best for unpredictable trajectories like bouncing or homing projectiles.

2. **Direction-Only Replication** - Only the initial spawn parameters (position, direction, speed) are replicated. The client simulates the rest of the trajectory locally. Perfect for linear projectiles where the path is predictable.

3. **Ring Buffer Replication** - Projectile spawn information is stored in a ring buffer on the Weapon component. The buffer contains spawn tick, position, and direction for efficient batched replication.

**Controls:**
- `E` - Cycle through replication methods

### Game Replication Modes (Rooms)

Six different networking approaches using lightyear's room system:

1. **All Predicted** (Room 0) - Current default mode: all entities predicted, server handles hit detection. Favors the target since the shooter might be mispredicting enemy positions.

2. **Client Predicted (No Lag Comp)** (Room 1) - Client predicted, enemies interpolated, no lag compensation. Shooter must lead targets (Quake-style gameplay).

3. **Client Predicted (Lag Comp)** (Room 2) - Client predicted, enemies interpolated, with lag compensation. Server rewinds enemy positions to validate hits from the client's perspective. Favors the shooter.

4. **Client-Side Hit Detection** (Room 3) - Hits computed on client and sent to server. Very cheap for server but vulnerable to cheating.

5. **All Interpolated** (Room 4) - Everything in interpolated timeline. User actions have built-in delay but everything is consistent.

6. **Only Inputs Replicated** (Room 5) - Only input states are replicated, everything else is predicted/simulated. Most bandwidth efficient.

**Controls:**
- `R` - Cycle through replication modes (changes room)

### Technical Implementation

#### Prespawning

When you have a player-controlled predicted entity (the predicted `Player` in this example),
it can be useful to be able to spawn objects (here `Bullets`) directly in the predicted timeline.

You can achieve this by having a system that runs both on the client and server and spawns the same entity. The entity should have the `PreSpawnedPlayerObject`. That entity will be spawned
immediately on the client in the predicted timeline. When the server spawns the entity, it will try replicating it to the client. There will be a matching step where that server entity will try to
match with a prespawned client entity (using the spawn `Tick` and the entity's `Archetype`). After the matching is done, the bullet becomes a normal `Predicted` entity.

#### Hit Detection

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

#### Room-based Interest Management

The example uses lightyear's room system to implement different replication strategies. Each replication mode corresponds to a different room, and players can switch between rooms to experience different networking behaviors.

https://github.com/user-attachments/assets/17bc985d-f700-439d-ba48-4c69fbfd7885



## Running an example

- Run the server with a gui: `cargo run -- server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,udp,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands to generate a self-signed certificate:
- `cd "$(git rev-parse --show-toplevel)" && sh certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
