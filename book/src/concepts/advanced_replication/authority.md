# Authority

Authority answers one question: who is allowed to decide the real state of an entity?

For now, the practical answer in Lightyear is the server. Entity replication is server to client. Clients can predict, render, interpolate, and send intent, but they should not be treated as authoritative replication senders.

That gives the usual server-authoritative flow:

1. The client sends inputs or a message.
2. The server validates the intent.
3. The server mutates the ECS world.
4. The server replicates the result to clients.
5. Clients reconcile or smooth what they receive.

This is the model to use for player movement, projectiles, inventory changes, doors, health, scoring, and most other gameplay state.

## What about authority components?

The codebase still has authority-related types such as `ControlledBy`, `HasAuthority`, and authority broker plumbing. They are useful for tracking ownership and for host-server/internal flows, but client-authoritative entity replication is not a supported path right now.

The reason is not philosophical. The current backend uses `bevy_replicon`, and the released Replicon path Lightyear is built on supports server-to-client entity replication. Until client-to-server entity replication is integrated properly, authority transfer that relies on clients sending component updates should be considered unfinished.

## Recommended pattern

Give each player a server-owned entity and attach ownership metadata:

```rust,ignore
commands.spawn((
    PlayerBundle::new(client_id),
    Replicate::to_clients(NetworkTarget::All),
    ControlledBy {
        owner: client_link_entity,
        lifetime: Default::default(),
    },
));
```

Then use `ControlledBy` to decide which input stream applies to which entity. The client controls the character by sending inputs, not by replicating the character.

For one-off client actions, send a message:

```rust,ignore
#[derive(Serialize, Deserialize, Clone)]
pub struct RequestSpawnProjectile {
    pub origin: Vec2,
    pub direction: Vec2,
}
```

The server can reject, clamp, rate-limit, or accept the request. If it accepts, it spawns a server entity with `Replicate`.

## When client authority lands

Client authority needs more than a marker component. It needs send-side entity mapping, conflict handling, validation, rebroadcasting, and clear rules for in-flight updates during authority changes.

Until those pieces are finished, prefer the boring model: clients send intent, the server owns state.
