# FPS

- there are still rollbacks on the remote client whenever a client shoots a bullet
- regularly the game spazzes out; maybe multiple rollbacks in a row?

- Bullets from the host-client disappear immediately upon collision, but not bullets from the remote client
- Bullets from the remote-client don't seem to take into account LagCompensation correctly for collisions

# Lobby


- server-hosted lobby: i can get in a situation where the client is spazzing out even after i stop moving. Infinite rollbacks? Incorrect timeline sync?


# Projectiles

- the cursor movement (that changes the direction) is not propagated to the server

Full entity replication mode
- hitscan weapon visuals don't appear on the client
- with linear projectile, sometimes 2 bullets are fired instead of 1

Direction only replication
- hitscan weapon visuals appear
- with linear projectile, sometimes 2 bullets are fired instead of 1

Client predicted (no lag comp): the bot moves offscreen instead of moving left and right
Only inputs replicated: bot does not move

Inputs seem to break down after switching rooms too much.



# Spaceships

Issues:
- The remote client sees a projectile getting fired twice
- the projectile on the remote client starts from further away from the source then what is shown on the local client (firing)

# Deterministic replication

After disconnecting client 1 and reconnecting, I get this error:
```
thread 'main' (45786887) panicked at /Users/charles/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_replicon-0.39.4/src/client.rs:424:11:
Entity despawned: The entity with ID 188v0 is invalid; its index now has generation 1.
Note that interacting with a despawned entity is the most common cause of this error but there are others

    If you were attempting to apply a command to this entity,
    and want to handle this error gracefully, consider using `EntityCommands::queue_handled` or `queue_silenced`.
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
Encountered a panic in system `bevy_replicon::client::receive_replication`!
```

Also i tried to do:
- client 1 connects
- client 1 moves
- client 2 connects
and i get some checksum mismatch
so StateBasedCatchup does not work even though we have an example for it
