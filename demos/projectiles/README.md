# Projectiles Demo

This demo is a small client/server test bed for projectile networking. It deliberately separates four decisions that are often bundled together:

1. **Trajectory** — what type of projectile is fired?
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

### Network representation

- **State entity** — replicate the projectile entity and its current position. This works for trajectories whose state may change, but sends ongoing state updates.
- **Fire-data entity** — replicate only the firing tick, origin, direction, and trajectory. Each peer reconstructs the projectile locally and fast-forwards it to account for network delay. This saves bandwidth, but requires a deterministic trajectory that can be reconstructed from its initial state.
- **Shot buffer** — replicate a fixed 32-slot ring on the player instead of one network entity per shot. Each record contains a monotonic sequence and sparse fire data; peers consume records once and create local projectiles. Replicon diffs send only the changed slot, and authoritative linear impacts update the record's finish tick.

### Hit policy

- **Server current** — the server tests against current authoritative target positions. It is simple and cheat-resistant, but an interpolated target seen by a client is older than the server target.
- **Server rewound** — the server uses the firing client's interpolation delay and Lightyear/Avian lag compensation to test historical target positions. This better matches the shooter's view, at additional server CPU and history cost.
- **Client reported** — the firing client performs the test and sends a hit claim to the server. This is responsive and cheap for the server, but intentionally trusts the client and is therefore insecure.

### Client timeline

- **Owner predicted** — each client predicts its own player and interpolates remote players. This is the usual responsive-client setup and is the main case for server rewind.
- **All predicted** — clients predict every player. Everyone is presented near the current server timeline, but prediction errors for remote players must be corrected.
- **All interpolated** — clients interpolate every player, including the locally controlled one. The entities share a consistent delayed timeline, at the cost of local input latency.

The axes are intentionally independent. Some combinations are mainly educational—for example, server rewind is generally unnecessary when every relevant entity is already presented in the same timeline.

## Running

- Server with a window: `cargo run -p projectiles -- --headless=false server`
- Client with id 1: `cargo run -p projectiles -- client -c 1`
- Host client: `cargo run -p projectiles -- host-client -c 0`
- Headless server with the built-in bot client: `cargo run -p projectiles --no-default-features --features=server,client,webtransport,netcode -- server`
- Headless server without the bot: `cargo run -p projectiles --no-default-features --features=server,webtransport,netcode -- server`


### WebTransport/Wasm

Run `bevy run web`. The repository includes a pre-generated development certificate and digest. If it expires, regenerate it with `cargo run -p generate_certificate`, then rebuild the Wasm client so the new digest is embedded.