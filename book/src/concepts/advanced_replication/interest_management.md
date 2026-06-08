# Interest management

Interest management means only sending an entity to clients that need to know about it.

It saves bandwidth, but it is also a security boundary. If a client should not know about an enemy behind fog-of-war, do not replicate that enemy to the client and hope the renderer hides it.

Lightyear uses Replicon's visibility system for this.

## Target first, visibility second

`Replicate::to_clients(...)` is the broad target:

```rust,ignore
Replicate::to_clients(NetworkTarget::All)
```

Visibility is the fine-grained filter. You can hide or show an entity for a specific client link:

```rust,ignore
commands.lose_visibility(enemy_entity, client_link_entity);
commands.gain_visibility(enemy_entity, client_link_entity);
```

The `client_link_entity` is the server-side entity representing that connection, usually the entity with `ClientOf` and `ReplicationSender`.

Visibility changes also propagate through Lightyear's replicated hierarchy support, so hiding a replicated parent can hide its replicated children too.

## Rooms

Rooms are the simple built-in interest-management tool.

Add `RoomPlugin`, allocate room ids, then put clients and entities in rooms. If a client and entity share at least one room, the entity is visible to that client. If they stop sharing a room, the entity is no longer visible.

```rust,ignore
app.add_plugins(RoomPlugin);

let room = app.world_mut().resource_mut::<RoomAllocator>().allocate();

commands.entity(client_link_entity).insert(Rooms::single(room));
commands.spawn((
    EnemyBundle::default(),
    Replicate::to_clients(NetworkTarget::All),
    Rooms::single(room),
));
```

This fits lobbies, map chunks, dungeon rooms, streaming zones, and other semi-static groups.

For fast-moving spatial relevance, you can still build your own system that calls `gain_visibility` and `lose_visibility` based on distance, teams, line of sight, or fog-of-war.

## What the client sees

When an entity becomes invisible to a client, the client's remote entity is despawned. When it becomes visible again, it is spawned again from server state.

Design client-only presentation around that. If you need a fade-out, death animation, or "last known position" marker, use a local client-side entity for the presentation and let the replicated entity disappear when visibility is lost.
