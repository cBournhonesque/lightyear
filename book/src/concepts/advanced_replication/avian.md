# Avian physics

Lightyear's `lightyear_avian2d` and `lightyear_avian3d` integrations coordinate four versions of a physics pose:

- Avian's simulation state: `Position` and `Rotation`.
- Replicated and predicted state.
- The temporary visual state produced by frame interpolation and rollback correction.
- Bevy's local `Transform` and derived `GlobalTransform` used for rendering.

The difficult part is ownership. A value written for rendering in `PostUpdate` must not become the starting point of the next fixed simulation tick.

## Recommended replication mode

Use `AvianReplicationMode::Position { sync_to_transform: false }` unless the application deliberately treats `Transform` as gameplay state.

| Mode | Replicated and predicted | Frame interpolation and correction | Physics authority | Recommendation |
|---|---|---|---|---|
| `Position { sync_to_transform: false }` | `Position`, `Rotation` | `Position`, `Rotation` | `Position`, `Rotation` | Preferred |
| `Position { sync_to_transform: true }` | `Position`, `Rotation` | `Position`, `Rotation` | Synchronizes the physics pose to `Transform` for fixed-tick authoring, then imports edits | Use for transform-driven fixed gameplay with compact physics replication |
| `Transform` | `Transform` | `Transform` | `Transform` at the application boundary; Avian still uses `Position` and `Rotation` internally | Use for transform-driven applications |

`FrameInterpolate` is type-erased: adding the one marker to an entity enables all applicable registered component or bundle rules, so correction and frame interpolation operate directly on Avian's canonical `Position` and `Rotation`.

`Position` is not always the right choice. Prefer `Transform` when:

- local-space transform hierarchy data is the network API;
- scale or a non-physics translation axis is gameplay state that must be replicated as part of the same component.

Those are authority and data-model choices, not visual-smoothing requirements. A render hierarchy can still follow a body replicated in `Position` mode because the integration writes the final visual `Position` and `Rotation` to `Transform` before Bevy propagates transforms.

## Setup for Position mode

Register the physics components used for replication and prediction. Interpolation rules are reused by both network interpolation and frame interpolation. Correction also uses these rules to sample the corrected visual pose after rollback.

```rust,ignore
app.component::<Position>()
    .replicate()
    .predict()
    .add_linear_interpolation()
    .add_correction();

app.component::<Rotation>()
    .replicate()
    .predict()
    .add_linear_interpolation()
    .add_correction();
```

Install the integration and disable Avian's overlapping synchronization and interpolation plugins:

```rust,ignore
app.add_plugins(LightyearAvianPlugin {
    replication_mode: AvianReplicationMode::Position {
        sync_to_transform: false,
    },
    ..default()
});

app.add_plugins(
    PhysicsPlugins::default()
        .build()
        .disable::<PhysicsTransformPlugin>()
        .disable::<PhysicsInterpolationPlugin>(),
);
```

In this mode, spawn and move bodies through `Position` and `Rotation`:

```rust,ignore
commands.spawn((
    RigidBody::Kinematic,
    Position::from_xy(10.0, 20.0),
    Rotation::default(),
));
```

By default, automatic synchronization is one-way from `Position` and `Rotation` to `Transform`, and it runs once in `PostUpdate` after frame interpolation and visual correction. A change made only to `Transform` is intentionally not copied back into physics. This prevents a stale rendered transform from overwriting state restored by rollback.

This also applies to input-driven systems in `FixedUpdate`. In the default configuration they should update `Position`/`Rotation`, velocity, forces, or other Avian state. A `Transform` change made there is not imported into physics, and the final Position-to-Transform writeback can overwrite it.

To let fixed-tick gameplay author `Transform` while retaining compact `Position`/`Rotation` replication, enable the optional bridge:

```rust,ignore
app.add_plugins(LightyearAvianPlugin {
    replication_mode: AvianReplicationMode::Position {
        sync_to_transform: true,
    },
    ..default()
});
```

When `sync_to_transform` is `true`, Lightyear synchronizes the restored canonical `Position` and `Rotation` to `Transform` before `FixedUpdate`. Gameplay can therefore safely read and update `Transform` during `FixedUpdate`. Lightyear imports the authored transform in `FixedPostUpdate` before Avian physics, then writes the simulated pose back afterward. This also works with frame interpolation: the previous frame's visual transform is replaced with the canonical pose before gameplay can edit it.

This authority rule applies to rigid-body poses. A child collider without its own `RigidBody` still
uses its local `Transform`/`ColliderTransform` to describe its offset from the parent body.

To smooth a predicted entity between fixed ticks from the moment it spawns, add the type-erased marker:

```rust,ignore
fn add_frame_interpolation(
    trigger: On<Add, Predicted>,
    mut commands: Commands,
) {
    commands.entity(trigger.entity).insert(FrameInterpolate);
}
```

Registering correction installs `FrameInterpolationPlugin` automatically. The marker does not name `Position` or `Rotation`; Lightyear selects all applicable interpolation rules from the entity's archetype. Correction also adds the marker automatically when a rollback first stores `PreviousVisual<C>`, but adding it at spawn time enables continuous between-tick smoothing before the first correction. A higher-priority bundle rule can be registered when translation and rotation must be sampled together.

## Position mode system order

The following order assumes a client with prediction, frame interpolation, and correction. Pure servers run the fixed simulation and history/replication work but do not create predicted visual corrections.

### PreUpdate: receive and rollback

```text
ReplicationSystems::Receive
  -> RollbackSystems::Check
  -> RollbackSystems::Prepare
  -> RollbackSystems::Rollback
  -> RollbackSystems::EndRollback
```

When a received authoritative value requires rollback:

1. `Prepare` stores the pre-rollback rendered value in `PreviousVisual<C>` for components registered with correction, then restores the rollback state.
2. `Rollback` replays the fixed simulation to the current prediction tick.
3. Frame-history updates are skipped during replay; recording every intermediate replay tick would replace render history with discarded work.
4. `EndRollback` repairs `FrameInterpolationHistory<C>` from the corrected `PredictionHistory<C>`, samples the corrected visual value with the registered interpolation rule, and creates `VisualCorrection` from the old visual pose to that corrected sample.
5. The live component is restored to the corrected canonical value before `PreUpdate` ends.

### RunFixedMainLoop: restore before simulation

```text
FrameInterpolationSystems::Restore
  -> optional Position/Rotation to Transform
  -> optional Transform propagation
  -> zero or more FixedMain iterations
```

`Restore` copies each frame history's `current_value` back into the live `Position` and `Rotation`. The previous frame's interpolated or visually corrected pose is only a render value and must not enter fixed simulation.

With `Position { sync_to_transform: false }`, no transform synchronization runs here. With `Position { sync_to_transform: true }`, the restored canonical physics pose is copied to `Transform` and propagated before `FixedUpdate`. This is essential because the transform left by the previous `PostUpdate` is visual rather than canonical.

`RunFixedMainLoop` runs once per rendered frame even when it executes zero fixed ticks. The restore therefore also happens on render frames with no fixed simulation. When several fixed ticks are needed, canonical state is restored once before the first and then advanced normally by each tick.

### FixedPostUpdate: simulate, then record

For every fixed tick:

```text
optional Transform propagation and Transform to Position/Rotation
  -> PhysicsSystems::StepSimulation
  -> optional Position/Rotation to Transform
  -> { PredictionSystems::UpdateHistory
       FrameInterpolationSystems::Update }
```

The two history updates both observe the completed physics step. Prediction history stores canonical state for future rollback. Frame history shifts its old current sample to `previous_value` and records the new canonical value as `current_value`.

The optional transform-authoring bridge runs its import in `PhysicsSystems::Prepare` and its writeback in `PhysicsSystems::Writeback`. It deliberately gives the transform authored during `FixedUpdate` precedence. Avian's ordinary conflict resolution cannot be used for this import because frame interpolation correctly marks `Position` changed after the previous physics tick; Avian would otherwise interpret that visual change as newer physics authority and reject the transform edit.

If several fixed ticks run in one rendered frame, frame history ends with the last two fixed poses. If no fixed tick runs, it remains unchanged.

### PostUpdate: construct the rendered pose

```text
ReplicationSystems::Send
  -> FrameInterpolationSystems::Interpolate
  -> RollbackSystems::VisualCorrection
  -> PhysicsSystems::Writeback
  -> TransformSystems::Propagate
```

1. Replication sends canonical component state before visual systems temporarily change it.
2. Frame interpolation writes `Position` and `Rotation` between the previous and current fixed samples using `Time<Fixed>::overstep_fraction()`.
3. Visual correction adds and decays the remaining rollback error on top of that interpolated pose. It runs second so frame interpolation cannot overwrite the correction.
4. Frame interpolation updates Bevy change detection for `Position` and `Rotation`, so Avian's
   ordinary writeback observes the final visual physics pose and copies it to `Transform`. The set
   ordering makes this work on render frames that contain no fixed tick as well.
5. Bevy transform propagation updates `GlobalTransform` and render children.

The live physics components contain visual values after `PostUpdate`. That is intentional and lasts only until `FrameInterpolationSystems::Restore` at the start of the next `RunFixedMainLoop`.

## Visual correction setup

There are two related pieces:

- `FrameInterpolationPlugin` installs the restore, history-update, and visual-interpolation systems. Correction registration installs it automatically.
- `FrameInterpolate` opts an entity into the applicable registered rules and causes its frame-history components to be inserted. `PreviousVisual<C>` requires this marker, so rollback correction adds it automatically.

Visual correction is built on that same rule, history, and restore pipeline. Calling `.add_correction()` is therefore sufficient to install the infrastructure needed for correction. Add `FrameInterpolate` earlier only when the entity should also be smoothed continuously between fixed ticks before its first rollback.

| Desired behavior | Registration and components |
|---|---|
| Immediate rollback snap; no between-tick smoothing | Predict the components, but omit `.add_correction()` and `FrameInterpolate` |
| Between-tick smoothing only | Add an interpolation rule, `FrameInterpolationPlugin`, and `FrameInterpolate`; omit correction |
| Rollback correction, enabling frame interpolation on first rollback | Add an interpolation rule and correction |
| Between-tick smoothing from spawn plus rollback correction | Add an interpolation rule and correction, then add `FrameInterpolate` at spawn |

`Position` mode itself works without frame interpolation or correction. Correction still uses frame history to preserve canonical simulation state while a corrected visual value is rendered, but its registration now installs that machinery automatically.

## Transform mode

This mode treats `Transform` as the replicated application state but synchronizes it into Avian before physics and back out afterward.

```text
FixedPostUpdate:
  propagate Transform -> Transform to Position/Rotation
  -> physics -> Position/Rotation to Transform
  -> prediction history + frame history for Transform

PostUpdate:
  frame-interpolate Transform -> correct Transform -> propagate
```

Use it only when application systems are intentionally transform-driven. It sends and stores more state than the physics pose and makes local hierarchy semantics part of the replication contract.

## Manual synchronization

Setting `LightyearAvianPlugin::update_syncs_manually` disables the integration's automatic `Position`/`Rotation`/`Transform` synchronization, including the optional fixed-tick bridge requested by `Position { sync_to_transform: true }`. The mode still configures history and visual-system ordering and still performs other integration work, such as child-collider position updates.

When synchronization is manual, preserve the same invariant: fixed simulation must read canonical state, replication must send canonical state, and render-only interpolation or correction must be applied after sending and before transform propagation.
