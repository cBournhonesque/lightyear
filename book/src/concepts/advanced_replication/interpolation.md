# Interpolation

## Introduction
Interpolation means that we will store replicated entities in a buffer, and then interpolate between the last two states to get a smoother movement.

See this excellent explanation from Valve: [link](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking)
or this one from Gabriel Gambetta: [link](https://www.gabrielgambetta.com/entity-interpolation.html)


## Implementation

In lightyear, interpolation can be automatically managed for you.

When you spawn the entity on the server, add an `InterpolationTarget` to say which clients should interpolate it:

```rust,ignore
commands.spawn((
    Replicate::to_clients(NetworkTarget::All),
    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
));
```

This means that all clients except the one with id `client_id` will interpolate this entity.
There is only one entity on the receiving side: it gets an `Interpolated` marker, its live components hold the interpolated values, and a `ConfirmedHistory<C>` on the same entity buffers the authoritative snapshots for every interpolated component. Every frame the live value is re-sampled from that buffer, slightly in the past.
(The owning client usually gets a `Predicted` marker instead; see [prediction](./prediction.md).)

## Which components get interpolated

Not every registered component is interpolated, only the ones you opt in at registration:

```rust,ignore
app.component::<PlayerPosition>()
    .replicate()
    .add_linear_interpolation();
```

If your component implements bevy's `Ease` trait, `add_linear_interpolation` just works. For anything else, provide the interpolation function explicitly with `add_interpolation_with`:

```rust,ignore
app.component::<MyComponent>()
    .replicate()
    .add_interpolation_with(|start, end, t| {
        // your blending logic here
        start.lerp(end, t)
    });
```

The function signature is `LerpFn<C> = fn(start: C, other: C, t: f32) -> C`.

## Interpolation delay

Sampling "slightly in the past" is what makes interpolation robust to jitter: there are always two confirmed states to blend between. The per-client delay is tracked with an `InterpolationDelay` component (the server also uses it as an estimate for lag compensation).

Interpolation runs in the `Update` schedule (`InterpolationSystems::Prepare`, then `Interpolate`), after time sync has run, so the sampling point tracks the synchronized timeline.

## Custom interpolation

In some cases, the interpolation logic can be more complex than a simple linear blend per component.
For example, you might want to interpolate based on multiple components at once (a cubic spline using position, velocity and acceleration).

In those cases, register with `InterpolationFns::history_only` (which only maintains the history buffer) and add your own systems in `InterpolationSystems::Interpolate`, which runs after lightyear has prepared the histories.
