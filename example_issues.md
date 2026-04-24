# Auth

Client panics with 
thread 'main' (34627593) panicked at /Users/charles/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_replicon-0.39.4/src/client/server_mutate_ticks.rs:176:9:
expected at most 1 messages, but confirmed 2
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
Encountered a panic in system `bevy_replicon::client::receive_replication`!

# Avian 3d character

Fails in host-client mode
```
thread 'main' (34631700) panicked at /Users/charles/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_ecs-0.18.0/src/world/mod.rs:389:57:
called `Result::unwrap()` on an `Err` value: DuplicateRegistration(ComponentId(347), ComponentId(454))
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

# Avian physics

Same host-client error

# Bevy enhanced inputs

Client-server mode: inputs are still borked and player goes off screen

# Deterministic replication

On the client, the players suddenly disappear from the screen.

# FPS

Fails with 
```
Error when initializing schedule PhysicsSchedule: 2 pairs of systems with conflicting data access have indeterminate execution order. Consider adding `before`, `after`, or `ambiguous_with` relationships between these:
 -- compute_hit_lag_compensation (in set Collisions) and clear_moved_proxies
    conflict on: ["avian2d::collider_tree::ColliderTrees"]
 -- compute_hit_lag_compensation (in set Collisions) and update_solver_body_aabbs<Collider>
    conflict on: ["avian2d::collision::collider::ColliderAabb", "avian2d::collider_tree::ColliderTrees"]
```

# Lobby

For a lobby where the server is hosting: inputs were broken for one of the players.

For a lobby where one of the players is hosting: only one player entity is created for both clients.

# Network visibility

Broken in host-client mode with the same error

# Priority

Hasn't been porter to replicon's priority handling.

# Projectiles

Server Does not start 
```
thread 'main' (34644118) panicked at examples/projectiles/src/server.rs:245:13:
Error adding plugin bevy_input::InputPlugin: : plugin was already added in application
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
Encountered a panic when applying buffers for system `projectiles::server::bot::spawn_bots`!
```

# Replication groups
Can we prevent the snake from going back from its current direction?
It would prevent panics when the snake disappears.

# Simple box

Client-server: it's completely broken at the beginning but gradually becomes better.
This probably means there is an issue with the timeline sync logic?

# Spaceships

No projectiles are shot
