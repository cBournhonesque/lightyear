# FPS

- Bullets from the remote client seem to briefly go back in time; I think the interpolation is doing something weird? There is no rollback though

# Lobby

- server-hosted game: the player entities are 'vibrating' instead of having a fixed position when no inputs are being sent
It seems to happen mostly on Predicted entities; what is the VisualPlayerPosition component?
The PlayerPosition component is fixed. Maybe it's due to FrameInterpolation?

# Projectiles

The example is broken; i don't see the bots, the input keys are not working, etc.

# Spaceships

Issues:
- bullets colliding with players are causing weird rollbacks, is it because that physics interaction is not predicted?
- Bullets from the remote client seem to briefly go back in time; I think the interpolation is doing something weird? There is no rollback though
- Sometimes bullets can 'go through' the circles
- the projectile on the remote client starts from further away from the source then what is shown on the local client (firing)

# Deterministic replication

I tried to do:
- client 1 connects
- client 1 moves
- client 2 connects
and i get some checksum mismatch so StateBasedCatchup does not work even though we have an example for it
