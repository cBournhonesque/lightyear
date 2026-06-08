# Visual interpolation

Visual interpolation smooths rendering between fixed simulation ticks.

It is not the same as network interpolation. Network interpolation smooths between server updates. Visual interpolation smooths between `FixedUpdate` and render frames on the same machine.

Most gameplay simulation should run in `FixedUpdate`. Rendering does not. If physics updates at 64 Hz and your monitor draws at 144 Hz, some rendered frames happen between simulation ticks. Without interpolation, movement can look uneven even when the simulation is correct.

Lightyear's current visual interpolation plugin is `FrameInterpolationPlugin`.

## Setup

First register an interpolation function for the component:

```rust,ignore
app.register_component::<Position>()
    .register_linear_interpolation();
```

Then add the frame interpolation plugin:

```rust,ignore
app.add_plugins(FrameInterpolationPlugin::<Position>::default());
```

Finally, opt entities in with `FrameInterpolate<C>`:

```rust,ignore
commands.spawn((
    PlayerBundle::default(),
    FrameInterpolate::<Position>::default(),
));
```

The plugin stores the previous and current fixed-tick values. During `PostUpdate`, it writes a visual value based on the fixed time overstep. Before the next fixed loop, it restores the real simulation value.

## Why restore?

The interpolated value is for presentation. It should not be fed back into physics, prediction, replication, or game logic.

That is why the plugin restores the real component value before simulation runs again. Your renderer sees smooth motion, while the simulation still sees exact fixed-tick state.

## With physics

If your renderer reads `Transform` but your physics writes `Position`/`Rotation`, make sure the physics-to-transform sync runs after frame interpolation. With Avian, that usually means running the sync plugin in `PostUpdate` when physics itself runs in `FixedUpdate`.

The exact setup depends on your physics mode, but the principle stays the same: interpolate the component that represents simulation state, then sync presentation from that interpolated value.
