# Bevy enhanced inputs

Client-server: inputs don't work.
Host-client: also seems broken.

Also got a panic with:
```
2026-04-27T22:49:05.862284Z ERROR lightyear_prediction::rollback: missing authoritative checkpoint mapping for ConfirmHistory entity=169v0 replicon_tick=RepliconTick(183)

thread 'Compute Task Pool (1)' (44110833) panicked at lightyear_prediction/src/rollback.rs:510:29:
missing authoritative checkpoint mapping for ConfirmHistory
```

# Deterministic replication

Even with two clients joining before any movement, there are big desyncs.
The simulation is not deterministic.

# FPS

Client-server: The movement seems to go too fast and are not totally smooth.

Prespawned bullets is broken: bullets seem to be spawned aat (0, 0), and trigger spurious rollbacks.
Normally everything should be smooth.

# Lobby

For a lobby where the server is hosting: inputs were broken for one of the players.
(it seems like 2 movement systems are running)?

For a lobby where one of the players is hosting: the non-host client gets very jittery replicated movement,
as if the interpolation is not working

# Priority

TODO: (not now, later) port to replicon's priority handling

# Projectiles

Server panics with 
```
thread 'Compute Task Pool (0)' (44104696) panicked at lightyear_prediction/src/rollback.rs:510:29:
missing authoritative checkpoint mapping for ConfirmHistory
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

Moving the cursor doesn't move the direction of the player.

There is no score displayed.

# Replication groups

Also pressing the direction opposite to the direction of the snake should do nothing instead of moving forward.

# Simple box

Client-server: the initial movements are still replicated in a very delayed manner. Timeline sync issue?

# Spaceships
Is input delay enabled?
Issues:
- Projectiles last for a short amount of time after a collision.
- Projectiles are jittery on the remote client (the client not shooting).
