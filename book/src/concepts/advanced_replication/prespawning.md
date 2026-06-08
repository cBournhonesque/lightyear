# Prespawning

Prespawning means the receiving side creates an entity before the replicated spawn for that entity arrives, then matches the local entity to the replicated entity instead of creating a duplicate.

That is useful for prediction, but it is not only a prediction feature.

You can use prespawning whenever both sides can create "the same" entity ahead of replication:

- a board game where both client and server spawn the board locally
- map objects loaded from the same level file
- action entities used by an input library
- projectiles or effects that the client wants to see immediately
- deterministic entities that are cheaper to spawn locally than to fully describe over the network

The important part is the match. The client and server need a deterministic signature that identifies the entity.

Replicon has this concept directly in its [`Signature`](https://docs.rs/bevy_replicon/latest/bevy_replicon/shared/replication/signature/struct.Signature.html) component. The short version is: insert a compatible signature on both the server entity and the pre-existing client entity. When the replicated spawn arrives, Replicon can map the server entity to the existing local entity if the signature matches.

Lightyear's `PreSpawned` component is a convenience layer around that matching flow. When `PreSpawned` is inserted on a local entity, Lightyear registers the matching hash and inserts the Replicon `Signature` for you.

## Why this exists

Normally, when the client receives a replicated server entity, it allocates a new local entity and stores the server-to-client mapping.

That is exactly right for most gameplay. The client had no local entity before, so it needs one.

Prespawning changes that first step. The local entity already exists. When the server spawn arrives, the client wants to say: "that server entity is this local entity."

Once that mapping exists, later replicated components and entity references can use the normal entity-map path.

## A non-prediction example

Imagine a chess board. The board squares are deterministic: every client and the server can spawn the same 64 squares from the same rules.

There is no need to replicate the full spawn for every square just to create local entities. Both sides can spawn them on startup. Then the server can replicate state for those entities, and signatures let the client match each server square to the square it already created.

The signature could be based on a component like:

```rust,ignore
#[derive(Component, Serialize, Deserialize, Hash)]
pub struct BoardSquare {
    pub file: u8,
    pub rank: u8,
}

commands.spawn((
    BoardSquare { file, rank },
    Signature::of::<BoardSquare>(),
));
```

On the server, the entity still needs to be replicated, for example with `Replicate::to_clients(NetworkTarget::All)`. On the client, the local pre-existing square only needs the matching data and signature.

The values must be unique per entity and identical on both sides. If two entities get the same signature, the mapping is ambiguous. If the same entity gets different signatures, it will not match.

You can also express this through Lightyear's `PreSpawned` component when you want Lightyear to track cleanup and rollback behavior:

```rust,ignore
commands.spawn((
    BoardSquare { file, rank },
    PreSpawned::new(board_square_hash(file, rank)),
));
```

## A prediction example

The usual game example is a projectile.

If the player presses fire, waiting for a round trip before showing the projectile can feel bad. The client can spawn a local projectile immediately. The server receives the input, validates it, and spawns the authoritative projectile. When that authoritative projectile replicates back, the client tries to match it to the local projectile.

That gives quick local feedback while keeping the server authoritative.

The tricky part is cleanup. The server may reject the shot, spawn it with different data, or the match may fail. Your game needs a policy for the local entity in those cases.

## What makes a good signature?

A good signature is:

- deterministic on both sides
- unique among entities that may be matched
- based on stable gameplay data, not local entity ids
- available before replication tries to match the entity

Good inputs include level ids, grid coordinates, spawn tick plus owner id, action name plus owning context, or a server/client-agreed spawn sequence.

Bad inputs include random local ids, floating-point values that may diverge, or data that only one side knows.

## `PreSpawned` hashes

`PreSpawned::new(hash)` is the explicit path. Use it when your game already has a stable id for the entity.

`PreSpawned::default()` computes a hash from the local spawn tick and the entity's registered component set. `PreSpawned::default_with_salt(salt)` adds an extra value to that hash. The salt is useful when two otherwise similar entities can be spawned on the same tick, such as two players firing the same projectile.

The default hash is convenient for prediction-style spawns, but it is not magic. Both sides still need to create compatible entities at compatible ticks. For deterministic world objects, an explicit hash is usually easier to reason about.

## Current Lightyear guidance

Prespawning is a good fit for deterministic remote matching and for latency hiding. It is also one of the areas most sensitive to the Replicon integration, because entity mapping has to be correct before later components with `Entity` fields are applied.

For ordinary gameplay entities, start with server-spawned replication. Add prespawning when you have a concrete reason:

- you need immediate local feedback
- the entity is deterministic and already exists locally
- an integration layer needs matching entities on both sides

When using it for prediction, remember that prespawning is only the spawn/match piece. Prediction still needs input history, component history, rollback, and deterministic fixed-tick systems.
