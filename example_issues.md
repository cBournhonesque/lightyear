# Bevy enhanced inputs

Occasional prediction glitches: timeline sync error, or the inputs did not arrive on time on the server?

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

Saw this panic:
```
thread 'Compute Task Pool (4)' (44375415) panicked at lightyear_prediction/src/rollback.rs:512:29:
missing authoritative checkpoint mapping for ConfirmHistory
```
Client-server: The movement seems to go too fast and are not totally smooth.

Prespawned bullets is broken: bullets seem to be spawned at (0, 0), and trigger spurious rollbacks.
Normally everything should be smooth.

# Lobby

For a lobby where one of the players is hosting: the non-host client sometimes gets very jittery replicated movement,
Issue with timeline sync? what is causing the excessive rollback?


# Projectiles

Server panics with 
```
thread 'Compute Task Pool (0)' (44104696) panicked at lightyear_prediction/src/rollback.rs:510:29:
missing authoritative checkpoint mapping for ConfirmHistory
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

Moving the cursor doesn't move the direction of the player.

There is no score displayed.



# Spaceships

The prespawned bullets are smooth, good job!

Issues:
- Projectiles last for a short amount of time after a collision. They should disappear immediately
- Projectiles are not displayed on remote client
- Projectiles don't push the balls, normally they should
- Score does not update on hit
