# Projectiles Example

https://github.com/user-attachments/assets/6705ed0e-bde4-4fc7-8b01-1a99f6cd748e


## Objective

This example explores the various tradeoffs between different approaches to replicating projectiles for a FPS game.
It can be time-consuming to test all these approaches; this should provide a convenient way to explore these approaches.

On the right is the server, which spawns 6 differents entities for each player. We use interest management via Rooms to make sure that clients only receive updates about entities that correspond to one of the 6 modes:
- **AllPredicted**: entity is predicted by all players, hit detection is done server-side with no lag compensation. This should favor the target since the shooter has an imperfect view of their 
position. This also allows testing that remote entity prediction works well with BEI, which is now the case.
- **ClientPredicted (No lag Comp)**: the 'default' setting of predicting the client and interpolating other players. Hit detection is done server-side; since there is no lag compensation, even if you 
  seemingly hit the target on the client's screen it won't be registered as a hit on the server because the client has a delayed view of other clients
- **ClientPredicted (Lag Comp)**: same as above but this time we use lag compensation on the server. The white boxes on the server are the collider occupied by each player in the last few frames. If 
  the projectile collides with that (broad-phase), we check if there was an actual collision on the client's screen using the narrow-phase lag compensation query. It's interesting to see how the boxes grow after any diagonal movement, but I think that's expected (it's just for broad-detection)
- **ClientSideHitDetection**: same as above, but hit detection is done on the client. This should give good results + saves a lot of server-CPU (since server doesn't need to do any hit-detection), 
  but cheaters are free to send fake 'I hit that target' packets
- **AllInterpolated**: this time the local client is also interpolated, so each of their movement will be felt after a delay. That's a tradeoff to make in exchange for having all entities in the same 
  timeline, which removes the need for lag compensation. Note that interpolation seems somewhat clunky because we interpolate between infrequent states without having a full view of the history of each component. I'm planning of maybe adding a mode where the full history of the component is replicated, for better interpolation.
- **OnlyInputsReplicated**: this time the server does basically nothing except acting as a proxy that exchanges inputs between players. The simulation should be deterministic and run on each client.
  If there is enough input-delay (for example in lockstep), each client has a perfect view of other clients and there is 0 prediction. Otherwise the client has to predict other players, which can make things tricky. How do we handle mispredicting that a target was shot? How do we handle receiving a late input telling us that a remote client shot a bullet?
  

The example also uses a fake 'bot' client that acts exactly like another client but runs in a headless app and communicates with the server using crossbeam channels. This is useful for testing to be able to see how remote clients work without having to manually spawn 2 clients.

## Features

This example showcases several features that can be useful for building a multiplayer FPS, including different weapon types, projectile replication strategies, and networking approaches.

**Controls:**
- `Q` - Cycle through weapon types
- `E` - Cycle through projectile replication modes
- `R` - Cycle through game replication modes
- `Space` - Shoot current weapon
  
### Weapon Types

The example now supports multiple weapon types that you can cycle through:

1. **Hitscan** - Instant hit
2. **Linear Projectile** - Simple projectile with constant velocity
3. **Shotgun** - Multiple pellets with spread
4. **Physics Projectile** - Projectile with physics interactions (bouncing, deceleration)
5. **Homing Missile** - Projectile that tracks nearest target

### Projectile Replication Methods

Three different approaches to replicating projectiles:

1. **Full Entity Replication** - Traditional approach where the entire projectile entity is replicated with regular updates. Best for unpredictable trajectories like bouncing or homing projectiles.

2. **Direction-Only Replication** - Only the initial spawn parameters (position, direction, speed) are replicated. The client simulates the rest of the trajectory locally. Perfect for linear projectiles where the path is predictable.

3. **Ring Buffer Replication** - Projectile spawn information is stored in a ring buffer on the Weapon component. The buffer contains spawn tick, position, and direction for efficient batched replication.


### Game Replication Modes (Rooms)

Six different networking approaches using lightyear's room system:

1. **All Predicted** (Room 0) - All entities predicted, server handles hit detection. Favors the target since the shooter might be mispredicting enemy positions.

2. **Client Predicted (No Lag Comp)** (Room 1) - Client predicted, enemies interpolated, no lag compensation. Shooter must lead targets (Quake-style gameplay).

3. **Client Predicted (Lag Comp)** (Room 2) - Client predicted, enemies interpolated, with lag compensation. Server rewinds enemy positions to validate hits from the client's perspective. Favors the shooter.

4. **Client-Side Hit Detection** (Room 3) - Client predicted, enemies interpolated. Hits computed on client and sent to server. Very cheap for server but vulnerable to cheating.

5. **All Interpolated** (Room 4) - Everything in interpolated timeline. User actions have built-in delay but everything is consistent (client and enemies are in the same interpolated timeline).

6. **Only Inputs Replicated** (Room 5) - Only inputs are replicated, otherwise only the clients are running the simulation, which needs to be perfectly deterministic. Most bandwidth efficient.


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
