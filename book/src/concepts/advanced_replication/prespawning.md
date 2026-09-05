# Prespawning

## Introduction

There are two ways to get a predicted entity on the client:
- normal ("delayed") predicted entities: they are spawned on the server and then replicated to the client.
  The client marks the received entity `Predicted` and starts simulating it ahead of the server.
- prespawned entities: the entity is created on the client (in the predicted timeline) and on the server using the same system.
  When the server replicates the entity back to the client, instead of treating it as a brand-new entity,
  the client matches it (by hash) with the pre-spawned one and keeps simulating that one.

This section focuses on prespawned entities.

## How does it work

You can find an example of prespawning in the [fps example](https://github.com/cBournhonesque/lightyear/tree/main/examples/fps), where bullets are prespawned on the client.

Let's say you want to spawn a bullet when the client shoots.
You could just spawn the bullet on the server and wait for it to be replicated + predicted on the client.
However that would introduce a delay between clicking on the 'shoot' button and seeing the bullet spawned.

So instead you run the same system on the client to prespawn the bullet in the predicted timeline.
The only thing you need to do is add the `PreSpawned` component to the entity spawned (on both the client and server).

```rust,noplayground
commands.spawn((BulletBundle::default(), PreSpawned::default()));
```

That's it!
- The client will assign a hash to the entity, based on its components and the tick at which it was spawned.
  You can also override the hash (`PreSpawned::new(hash)`) or add a salt (`PreSpawned::default_with_salt(client_id)`) to tell apart entities spawned on the same tick by different players.
- When the client receives the server entity, it matches the signature against its prespawned entities.
  If it matches, it re-uses the prespawned entity as the `Predicted` entity instead of spawning a new one.
  If nothing matches, it just spawns a normal predicted entity.


## In-depth

The various pieces for prespawning are:

- `PreSpawned` component hook, on_add:
  - Unless a hash is provided, computes the hash of the prespawned entity based on its archetype (only the replicated components) + spawn tick.

- Matching happens through Replicon's signature mechanism: the prespawned entity's signature is compared with incoming server entities.
  If there is a match, the prespawned entity is kept as the predicted entity (marked `Predicted`) instead of spawning a fresh one.

- `PreSpawnedReceiver` is an app-global resource (not on the link) that tracks locally prespawned entities: their hashes, spawn ticks, and lifecycle. It also shifts them along on `LocalTimelineShift` so they stay consistent with the timeline.

- `PreSpawnedSystems::CleanUp`:
  - removes prespawned entities on the client that never got matched with any server entity (they time out).


One thing to note is that we updated the rollback logic for pre-spawned entities. The normal rollback logic is:
- we receive a confirmed update
- we check if the confirmed update matches the predicted history
- if not, we initiate a rollback, and restore the predicted history to the confirmed state. (Thanks to replication group, all components of all entities
  in the replication group are guaranteed to be on the same confirmed tick)

However for pre-spawned entities, we do not have any confirmed state yet! So instead we need to rollback to the history of the pre-spawned entity itself.
- we compute the prediction history of all components during FixedUpdate
- when we have a rollback, we also rollback all prespawned entities to their history
- Edge cases:
  - if the prespawned entity didn't exist at the rollback tick, we despawn it
  - if a component didn't exist at the rollback tick, we remove it
  - if a component existed at the rollback tick but not anymore, we re-spawn it
  - TODO: if the preentity existed at the rollback tick but not anymore, we re-spawn it
    This one is NOT handled (or maybe it is via `prediction_despawn()`, check!)


## Caveats

There are some things to be careful of:
- the entity must be spawned in a system that runs in the `FixedMain` schedule, because only then are you guaranteed
  to have exactly the same tick between client and server.
  - If you spawn the prespawned entity in the `Update` schedule, it won't be registered correctly for rollbacks, and also the tick associated
    with the entity spawn might be incorrect.
  
