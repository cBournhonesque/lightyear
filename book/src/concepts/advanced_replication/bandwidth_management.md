# Bandwidth management

The cheapest packet is the one you never send.

Before looking for clever prioritization, first make sure the server only replicates state that clients actually need.

## Replicate less

Do not replicate rendering-only data such as mesh handles, particle state, local animation helpers, UI markers, or client-only effects. Spawn those locally on the client when a replicated gameplay entity appears.

Prefer small gameplay components:

```rust,ignore
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct Position(pub Vec2);

#[derive(Component, Serialize, Deserialize, Clone)]
pub struct Health(pub u16);
```

Leave large local presentation state out of the replication registry.

## Use `register_component_once`

Some components need to be sent when the entity appears, but do not need later mutation updates.

```rust,ignore
app.register_component_once::<PlayerId>();
app.register_component_once::<Team>();
```

That is a good fit for ids, tags, display names, team assignment, spawn metadata, and other mostly-static state.

## Use visibility

Visibility is the biggest bandwidth lever.

Use `NetworkTarget` for broad targeting and `gain_visibility` / `lose_visibility` or `Rooms` for interest management. If a client cannot see or interact with an entity, do not replicate that entity to the client.

```rust,ignore
commands.lose_visibility(entity, client_link);
```

For large worlds, divide the world into rooms or zones and move clients/entities between them as needed.

## Tune send intervals carefully

The send interval controls how often pending network data is flushed. Higher rates feel more responsive but cost more bandwidth and CPU. Lower rates are cheaper but make remote entities update less often, so interpolation becomes more important.

Start with a conservative rate that works for your game, then measure.

## Priority

For now, the reliable tools are component choice, one-shot registration, visibility, and send rate.

Channel priorities still matter for user messages that flow through transport channels. Replicated entity updates are currently driven by the Replicon backend, so entity-level replication priority should be treated as a separate design area rather than something you get automatically from channel priority.
