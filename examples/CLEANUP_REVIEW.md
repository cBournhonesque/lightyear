# Cleanup Review Notes

Items below looked non-obvious during the examples cleanup and should be reviewed before changing behavior.

- `examples/projectiles/src/shared.rs`: several TODOs around client-side hit prediction, server re-replication of bullets, projectile-spawn correction, and interpolation-tick timing need a replication design decision.
- `examples/projectiles/src/shared.rs`: `projectile_prespawn_salt` and `PreSpawned::default_with_salt` are still used for multi-projectile shots, such as shotgun pellets. Unlike the FPS example, these can create multiple entities for the same player and tick.
- `examples/fps/src/server.rs`: the hit checks still use `BULLET_COLLISION_DISTANCE_CHECK`; deciding whether this should depend on velocity length affects hit detection behavior.
- `examples/replication_groups/src/client.rs`: the missing-parent case during custom interpolation may indicate sync timing issues, but changing it needs a reproduction.
- `examples/network_visibility/src/client.rs`: the TODO about making prediction mode a separate component is architectural and not just cleanup.
- `examples/launcher`: several TODOs cover WebTransport certificate UI, link-conditioner settings, and dynamic example discovery. These are feature gaps rather than stale comments.
