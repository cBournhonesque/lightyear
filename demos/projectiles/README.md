# Projectiles Demo

This demo is a small client/server test bed for projectile networking. It deliberately separates four decisions that are often bundled together:

1. **Trajectory** — what path does the shot follow?
2. **Representation** — what projectile data is sent over the network?
3. **Hit policy** — where and when is a hit decided?
4. **Timeline** — which entities are predicted or interpolated on clients?

There is one arena and one player per connected peer. Changing any axis resets the arena by destroying the current players and projectiles, then recreating them. This makes each test start from a known state and avoids hiding behavior behind room-based interest management.

## Controls

- `WASD` — move
- Mouse — aim
- `Space` — shoot
- `Q` — cycle trajectory
- `E` — cycle network representation
- `R` — cycle hit policy
- `T` — cycle client timeline

The four selected values are displayed on screen and replicated by the server.

## Axes

### Trajectory

- **Hitscan** — an instantaneous ray cast with a short-lived visual trace.
- **Linear** — a projectile moving at a fixed speed in a straight line.

Trajectory code lives in `src/trajectory/`.

### Network representation

- **State entity** — replicate the projectile entity and its current position. This works for trajectories whose state may change, but sends ongoing state updates.
- **Fire-data entity** — replicate only the firing tick, origin, direction, and trajectory. Each peer reconstructs the projectile locally and fast-forwards it to account for network delay. This saves bandwidth, but requires a deterministic trajectory that can be reconstructed from its initial state.
- **Shot buffer** — replicate a fixed 32-slot ring on the player instead of one network entity per shot. Each record contains a monotonic sequence and sparse fire data; peers consume records once and create local projectiles. Replicon diffs send only the changed slot, and authoritative linear impacts update the record's finish tick.

Representation code lives in `src/representation/`.

### Hit policy

- **Server current** — the server tests against current authoritative target positions. It is simple and cheat-resistant, but an interpolated target seen by a client is older than the server target.
- **Server rewound** — the server uses the firing client's interpolation delay and Lightyear/Avian lag compensation to test historical target positions. This better matches the shooter's view, at additional server CPU and history cost.
- **Client reported** — the firing client performs the test and sends a hit claim to the server. This is responsive and cheap for the server, but intentionally trusts the client and is therefore insecure.

Hit-policy code lives in `src/hit_detection/`.

### Client timeline

- **Owner predicted** — each client predicts its own player and interpolates remote players. This is the usual responsive-client setup and is the main case for server rewind.
- **All predicted** — clients predict every player. Everyone is presented near the current server timeline, but prediction errors for remote players must be corrected.
- **All interpolated** — clients interpolate every player, including the locally controlled one. The entities share a consistent delayed timeline, at the cost of local input latency.

Timeline code lives in `src/timeline/`.

The axes are intentionally independent. Some combinations are mainly educational—for example, server rewind is generally unnecessary when every relevant entity is already presented in the same timeline.

## Firing and hit detection

A predicted owner creates the shot immediately. The server independently validates firing cadence. For `StateEntity`, the owner projectile and server projectile use the same deterministic `PreSpawned` signature. For `FireDataEntity`, the fire-data network entity is matched and owns a non-networked local visual child. `ShotBuffer` instead predicts a write to the player's replicated ring; no per-shot network entity or prespawn matching is involved. Its sequence is ring bookkeeping, not a dedicated shot-identity entity.

Linear hit detection stores a local `ProjectileSweepStart` and casts over the complete segment from that point to the projectile's newly simulated or interpolated position. The start is advanced after the segment is checked, so a fast projectile cannot pass through a target between samples. This is collision bookkeeping, not replicated projectile state.

Projectile lifetime is expressed in fixed ticks. Reconstructed projectiles use the local simulation tick for authoritative/predicted entities and the interpolation tick for interpolated entities. A successful hit also leaves a short-lived local red cross at the exact impact point in whichever app performed collision detection.

Client-reported hit detection casts the player rectangle directly against the player poses rendered by that client. It does not add those render-only replicas to Avian's physics world, which keeps client rollback and arena resets independent of collision-debug state.

`RigidBody` is also local simulation setup rather than replicated state. The server owns its authoritative bodies, predicted clients derive a kinematic body when they receive a simulated player or linear state projectile, and interpolated entities never receive one. Besides keeping delayed presentation out of Avian's solver, this prevents `RigidBody` from inserting a temporary default pose that would flash a new interpolated projectile at the world origin.

Players move through Avian `LinearVelocity`, rather than editing `Position` while reporting zero velocity. Predicted players receive frame interpolation between fixed ticks; remote interpolated players use the replicated positions and matching velocities to interpolate continuously between the server's 100 ms snapshots.

For server rewind, Lightyear's `LagCompensationSpatialQuery` evaluates target colliders at the historical time corresponding to the firing client's view. A GUI server draws every sampled target collider briefly in yellow, including queries that miss. Broad-phase history AABBs, current-physics collider gizmos, and current-to-rewound connector lines are hidden so the outline shows only the pose actually tested by the lag-compensated narrow phase. Useful background:

- [Valve: Lag Compensation](https://developer.valvesoftware.com/wiki/Lag_Compensation)
- [Gabriel Gambetta: Lag Compensation](https://gabrielgambetta.com/lag-compensation.html)

## Running

- Server with a window: `cargo run -p projectiles -- --headless=false server`
- Client with id 1: `cargo run -p projectiles -- client -c 1`
- Host client: `cargo run -p projectiles -- host-client -c 0`
- Headless server with the built-in bot client: `cargo run -p projectiles --no-default-features --features=server,client,webtransport,netcode -- server`
- Headless server without the bot: `cargo run -p projectiles --no-default-features --features=server,webtransport,netcode -- server`

The built-in bot is a normal headless client connected through local channels. It starts above the primary player facing down, strafes horizontally without rotating, and fires every two seconds. The primary player starts below it facing up; additional players use nearby horizontal lanes. The bot's nested app, pacing, and input behavior live in `src/bot.rs`; `server.rs` only wires it into the server.

The initial axes can also be selected with `LIGHTYEAR_INITIAL_TRAJECTORY`, `LIGHTYEAR_INITIAL_REPRESENTATION`, `LIGHTYEAR_INITIAL_HIT_POLICY`, and `LIGHTYEAR_INITIAL_TIMELINE`.

### WebTransport/Wasm

Run `bevy run web`. The repository includes a pre-generated development certificate and digest. If it expires, regenerate it with `cargo run -p generate_certificate`, then rebuild the Wasm client so the new digest is embedded.
