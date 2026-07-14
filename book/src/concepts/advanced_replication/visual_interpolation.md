# Frame interpolation

Game simulation normally runs in Bevy's fixed schedules so its rate does not depend on rendering FPS. Render frames and fixed ticks do not line up exactly, however: a rendered frame can contain zero, one, or several fixed ticks. Rendering the latest fixed pose directly therefore looks jittery.

Lightyear's `FrameInterpolationPlugin` smooths this by rendering one fixed tick behind the simulation:

```text
lerp(previous_fixed_value, current_fixed_value, overstep_fraction)
```

This is separate from network interpolation. Network interpolation samples buffered server snapshots on an interpolated timeline. Frame interpolation smooths values between local fixed ticks, including values produced by client prediction.

## Setup

First register an interpolation rule. Rules registered for network interpolation are reused:

```rust,ignore
app.component::<Position>()
    .replicate()
    .predict()
    .add_linear_interpolation();
```

For a local-only component, register a rule directly:

```rust,ignore
app.interpolate_with::<MyPosition>(
    InterpolationFns::no_history(|start, end, t| {
        MyPosition(start.0.lerp(end.0, t))
    }),
);
```

Then add the plugin and opt entities in with `FrameInterpolate`:

```rust,ignore
app.add_plugins(FrameInterpolationPlugin);

fn enable_frame_interpolation(
    trigger: On<Add, Predicted>,
    mut commands: Commands,
) {
    commands.entity(trigger.entity).insert(FrameInterpolate);
}
```

`FrameInterpolate` is type-erased. It enables every applicable registered component or bundle rule on the entity; component-specific interpolation marker types are not needed. Lightyear automatically inserts the corresponding `FrameInterpolationHistory<C>` components when the marker and live components are both present.

Use `SkipFrameInterpolation` for a frame in which a discontinuity such as a teleport should not be interpolated.

## System order

Frame interpolation has three system sets:

```text
RunFixedMainLoop:
  FrameInterpolationSystems::Restore

FixedPostUpdate, after fixed simulation:
  FrameInterpolationSystems::Update

PostUpdate, after replication send and before transform propagation:
  FrameInterpolationSystems::Interpolate
```

### Restore

`PostUpdate` temporarily writes visual values into live components. Before any fixed ticks run on the next rendered frame, `Restore` copies `FrameInterpolationHistory<C>::current_value` back to the component. Fixed simulation therefore reads canonical state rather than the previous frame's rendered value.

The restore runs once per rendered frame before the fixed loop, including frames in which the loop executes no fixed tick.

### Update

After each fixed simulation tick, `Update` shifts the existing `current_value` to `previous_value` and records the new canonical live value as `current_value`. With several fixed ticks in one rendered frame, the history finishes with the last two fixed samples.

History updates are skipped during rollback replay. After replay, prediction repairs frame history from corrected prediction history instead of recording every discarded intermediate replay step.

### Interpolate

In `PostUpdate`, `Interpolate` samples the previous and current fixed values using `Time<Fixed>::overstep_fraction()`. It runs after replication sends so a temporary visual value is not replicated, and before Bevy transform propagation so rendering sees the smoothed pose.

Frame interpolation updates the live component through Bevy's change-detecting mutable access.
Downstream systems that filter on `Changed<C>` therefore observe the interpolated value in the
same `PostUpdate`, provided they run after `FrameInterpolationSystems::Interpolate`. The visual
value itself is not replicated: interpolation runs after replication sends, and the canonical
current fixed value is restored before the next render schedule.

## Interaction with visual correction

Prediction correction smooths the discontinuity caused by rollback. It uses the same interpolation rules and frame history:

```text
frame interpolation -> visual correction -> transform propagation
```

Frame interpolation first produces the corrected timeline's visual sample. `RollbackSystems::VisualCorrection` then adds and decays the difference from the pre-rollback rendered value. Reversing these systems would let frame interpolation overwrite correction.

Because correction shares this pipeline, registering `.add_correction()` installs `FrameInterpolationPlugin` automatically. When rollback stores `PreviousVisual<C>`, its required components add `FrameInterpolate` to the entity. Add the marker at spawn time only when continuous between-tick smoothing should begin before the first rollback. If rollback should snap immediately, omit correction.

## Avian physics

Avian adds synchronization between physics `Position`/`Rotation` and Bevy `Transform`, so its ordering contract is more specific. See [Avian physics](./avian.md) for the recommended replication mode, exact rollback/fixed/render order, transform authority, and supported visual-correction combinations.

## Tradeoffs

Frame interpolation:

- smooths rendering across variable render frame times;
- introduces one fixed tick of visual delay;
- stores previous and current values for each interpolated component;
- temporarily writes visual values into live components, making restore ordering essential.

An alternative is to simulate an extra partial tick for rendering. That avoids the one-tick delay but costs an additional simulation and must not commit partial-tick state to the canonical timeline.
