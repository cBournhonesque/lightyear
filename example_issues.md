# Avian physics

Seems to work (including host-client), apart from some weird artifacts at the beginning. Maybe the first rollback is weird?
Is the timeline sync ok? or we have too much prediction history?

# Bevy enhanced inputs

Client-server: inputs don't work.
Host-client: also seems broken.

# Deterministic replication

Even with two clients joining before any movement, there are big desyncs.

# FPS

Client-server: The movement seems to go too fast, maybe the movement system runs on both client and server?
(i compiled the binary with both client and server features enabled)

Prespawned bullets is broken: i see duplicates or errors.

# Lobby

For a lobby where the server is hosting: inputs were broken for one of the players.
(it seems like 2 movement systems are running)?

For a lobby where one of the players is hosting: the inputs don't work for the host.

# Network visibility

Host-client: inputs don't work on the host.

# Priority

TODO: (not now, later) port to replicon's priority handling

# Projectiles

On the client; moving the cursor works but pressing WASD doesn't work.
Also the other keyboard inputs (Q, etc.) don't work.

# Replication groups

Host-client: the host doesn't seem to be able to move their entity
Also pressing the direction opposite to the direction of the snake should do nothing instead of moving forward.

# Simple box

Client-server: the initial movements are still replicated in a very delayed manner. Timeline sync issue?
Host-client: the host doesn't seem to be able to move their entity

# Spaceships
The walls are jittering during rollbacks
Projectiles bounce on target instead of disappearing in an explosion
I get this error when shooting a bullet on a very close target:
```
thread 'main' (43908878) panicked at /Users/charles/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_ecs-0.18.0/src/error/handler.rs:125:1:
Encountered an error in command `<bevy_ecs::system::commands::entity_command::insert<lightyear_tools::debug::component::LightyearDebug>::{{closure}} as bevy_ecs::error::command_handling::CommandWithEntity<core::result::Result<(), bevy_ecs::world::error::EntityMutableFetchError>>>::with_entity::{{closure}}`: Entity despawned: The entity with ID 242v10 is invalid; its index now has generation 11.
```

