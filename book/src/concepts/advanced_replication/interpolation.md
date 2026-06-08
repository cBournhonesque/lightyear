# Interpolation

Network interpolation smooths entities that are controlled by the server.

The server does not send an update every render frame. It sends snapshots at a network rate, packets can arrive unevenly, and some packets can be lost. If the client draws the latest received value directly, movement can look jittery.

Interpolation fixes that by rendering a little bit behind the newest server state. The client keeps a small history of confirmed component values and samples between two known states.

## How to enable it

Register interpolation for the component:

```rust,ignore
app.register_component::<PlayerPosition>()
    .add_linear_interpolation();
```

Then tell Lightyear which clients should interpolate the entity:

```rust,ignore
commands.spawn((
    PlayerBundle::new(client_id),
    Replicate::to_clients(NetworkTarget::All),
    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
));
```

That example is the usual player setup: the owning client predicts its own player, and other clients interpolate it.

## What gets created?

Do not think of interpolation as a second replicated entity. The server still replicates one entity. On the client, `Interpolated` is a marker that changes how registered components are applied.

For an interpolated component, incoming authoritative values are stored in `ConfirmedHistory<C>`. The interpolation systems then write the sampled value to the live component for the interpolation timeline.

## Interpolation functions

For simple types, `.add_linear_interpolation()` is enough if the component supports Bevy's `Ease` behavior.

For custom types, provide your own function:

```rust,ignore
fn interpolate_position(start: PlayerPosition, end: PlayerPosition, t: f32) -> PlayerPosition {
    PlayerPosition(start.0.lerp(end.0, t))
}

app.register_component::<PlayerPosition>()
    .add_interpolation_with(interpolate_position);
```

If a component needs several fields or several entities to interpolate correctly, use `.add_custom_interpolation()`. Lightyear will keep the confirmed history for you, and your own system can decide how to sample it.

## Interpolation versus frame interpolation

Network interpolation smooths between server updates.

Frame interpolation smooths between fixed simulation ticks and render frames.

Most games need both eventually, but they solve different problems. If an entity is stuttering because packets arrive at 10 or 20 Hz, use network interpolation. If an entity is stuttering because physics runs in `FixedUpdate` and rendering runs in `Update`/`PostUpdate`, use frame interpolation.
