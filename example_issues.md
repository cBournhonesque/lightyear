# FPS

- Bullets are not smooth, are they not frame interpolated?
- Host-client: collisions from the host bullets don't work with the interpolated entity.
- Bullets should disppear immediately when colliding with the interpolated entity
- Client-server: at the beginning, movement is very choppy. Maybe because the time sync is not done yet?
- Client-server: bullets from the remote client (so replicated) start by appearing at (0, 0). Maybe some avian transform propagation issue?


# Lobby

- Server-hosted lobby: the timelines seem to take a long time to sync, the movement is very jittery for the first ~5 seconds after starting the game. Maybe the timelines needs to be reset when this happens?


# Projectiles

Full entity replication mode
- hitscan weapon visuals don't appear on the client
- with linear projectile, sometimes 2 bullets are fired instead of 1

Direction only replication
- hitscan weapon visuals appear
- with linear projectile, sometimes 2 bullets are fired instead of 1

Room Client Predicted (no lag comp or lag comp) / Client-Side prediction / All-Interpolated / Only Inputs Replicated: inputs don't work and bot is not moving

After changing rooms, inputs don't work. I see a lot of issues like
`2026-05-03T23:31:49.381736Z  WARN lightyear_inputs::client: Could not find entity in entity_map for remote player input message PreSpawned(6364136223846793007)`


# Spaceships

The prespawned bullets are smooth, good job!

Issues:
- The remote client sees a projectile getting fired twice
- the projectile on the remote client starts from further away from the source then what is shown on the local client (firing)

# Deterministic replication

The StateBasedCatchup is not working. Is it even enabled now?
Can we display which mode is being used on the screen? Maybe it wasn't enabled.

