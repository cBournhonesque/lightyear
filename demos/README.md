# Demos

This folder contains larger interactive Lightyear applications. Unlike the
focused examples, these demos combine several networking techniques in one
application and are intended for manual experimentation.

- [`projectiles`](projectiles/README.md) explores independent projectile
  trajectory, representation, hit-detection, and client-timeline choices.
- [`spaceships`](spaceships/README.md) demonstrates predicted multiplayer
  physics, prespawned bullets, input delay, and deterministic P2P play.

Both packages are workspace members. Run either one with `cargo run -p <name>`
or select it in the shared build recipe, for example:

```sh
just build_examples names=projectiles features=client,server
```
