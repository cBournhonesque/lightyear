# FPS

- Example is broken in host-client mode, the bullets get stuck shortly after being fired (both bullets from host or from client)

# Lobby

- server hosted game:
panics with
```
thread 'main' (69659296) panicked at /Users/charles/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_replicon-0.39.4/src/server.rs:306:29:
registry should always exist on the server
```

- server-hosted game: the player entities are 'vibrating' instead of having a fixed position when no inputs are being sent
It seems to happen mostly on Predicted entities; what is the VisualPlayerPosition component?
The PlayerPosition component is fixed. Maybe it's due to FrameInterpolation?

# Projectiles

The example is broken; i don't see the bots, the input keys are not working, etc.

# Spaceships

Issues:
- Sometimes bullets can 'go through' the circles

# Deterministic replication

I tried to do:
- client 1 connects
- client 1 moves
- client 2 connects
and i get some checksum mismatch so StateBasedCatchup does not work even though we have an example for it
Also got this panic:
```
thread 'main' (69666476) panicked at /Users/charles/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_replicon-0.39.4/src/client.rs:424:11:
Entity despawned: The entity with ID 189v0 is invalid; its index now has generation 1.
Note that interacting with a despawned entity is the most common cause of this error but there are others
```
especially wen reconnecting.

