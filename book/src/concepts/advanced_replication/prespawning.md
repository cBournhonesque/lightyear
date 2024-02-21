# Prespawning

## Introduction

There are several types of entities you might want to create on the client (predicted) timeline:
- normal ("delayed") predicted entities: they are spawned on the server and then replicated to the client.
  The client creates a Confirmed entity, and performs a rollback to create a corresponding Predicted entity on the client timeline.
- client-issued pre-predicted entities: the entity is first created on the client, and then replicated to the server.
  The server then receives the entity, decides if it's valid, and if so, replicates back the original client.
- prespawned entities: the entity is created on the client (in the predicted timeline) and the server using the same system. 
  When the server replicates the entity back to the client, the client creates a Confirmed entity, but instead of 
  creating a new Predicted entity, it re-uses the pre-spawned entity.

This section focuses about the third type of predicted entity: prespawned entities.

## How does it work

You can find an example of prespawning in the [prespawned example](https://github.com/cBournhonesque/lightyear/tree/main/examples/bullet_prespawn).

Let's say you want to spawn a bullet when the client shoots.
You could just spawn the bullet on the server and wait for it to be replicated + predicted on the client.
However that would introduce a delay between clicking on the 'shoot' button and seeing the bullet spawned.

So instead you run the same system on the client to prespawn the bullet in the predicted timeline.
The only thing you need to do is add the `PreSpawnedPlayerObject` component to the entity spawned (on both the client and server).

```rust,noplayground
commands.spawn((BulletBundle::default(), PreSpawnedPlayerObject));
```

That's it!
- The client will assign a hash to the entity, based on its components and the tick at which it was spawned.
  You can also override the hash to use a custom one.
- When the client receives a server entity that has `PreSpawnedPlayerObject`, it will check if the hash matches any of its pre-spawned entities.
  If it does, it will remove the `PreSpawnedPlayerObject` component and add the `Predicted` component.
  If it doesn't, it will just spawn a normal predicted entity.


## In-depth

The various system-sets for prespawning are:
- PreUpdate schedule:
  - `PredictionSet::SpawnPrediction`: we first run the prespawn match system to match the pre-spawned entities with their corresponding server entity.
    If there is a match, we remove the PreSpawnedPlayerObject component and add the Predicted/Confirmed components.
    We then run an apply_deferred, and we run the normal predicted spawn system, which will skip all confirmed entities that 
    already have a `predicted` counterpart (i.e. were matched)

- FixedUpdate schedule:
  - FixedUpdate::Main: prespawn the entity
  - FixedUpdate::SetPreSpawnedHash: we compute the hash of the prespawned entity based on its archetype (only the components that are present in the ComponentProtocol) + spawn tick.
     We store the hash and the spawn tick in the `PredictionManager` (not in the `PreSpawnedPlayerObject` component).
  - FixedUpdate::SpawnHistory: add a PredictionHistory for each component of the pre-spawned entity. We need this to:
    - not rollback immediately when we get the corresponding server entity
    - do rollbacks correctly for pre-spawned entities

- PostUpdate schedule:
  - we cleanup any pre-spawned entity no the clients that were not matched with any server entity.
    We do the cleanup when the `(spawn_tick - interpolation_tick) * 2` ticks have elapsed. Normally at interpolation tick we should have 
    received all the matching replication messages, but it doesn't seem like it's the case for some reason.. To be investigated.


One thing to note is that we updated the rollback logic for pre-spawned entities. The normal rollback logic is:
- we receive a confirmed update
- we check if the confirmed update matches the predicted history
- if not, we initiate a rollback, and restore the predicted history to the confirmed state. (Thanks to replication group, all components of all entities
  in the replication group are guaranteed to be on the same confirmed tick)

However for pre-spawned entities, we do not have a confirmed entity yet! So instead we need to rollback to history of the pre-spawned entity.
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
- the entity must be spawned in a system that runs in the `FixedUpdate::Main` SystemSet, because only then are you guaranteed 
  to have exactly the same tick between client and server.
  - If you spawn the prespawned entity in the `Update` schedule, it won't be registered correctly for rollbacks, and also the tick associated
    with the entity spawn might be incorrect.
  
