# Visual Interpolation

Usually, you will want your simulation (physics, etc.) to run during the `FixedUpdate` schedule.

The reason is that if you run this logic during the `Update` schedule, the simulation will run at a rate that is 
influenced by the frame rate of the client. This can lead to inconsistent results between clients: a beefy machine
might have a higher FPS, which would translate into a "faster" game/simulation.

This article by GafferOnGames talks a bit more about this: [Fix Your Timestep](https://gafferongames.com/post/fix_your_timestep/)


The issue is that this can cause the movement of your entity to appear jittery (if the entity only moves during the FixedUpdate schedule). There could be frames where the FixedUpdate schedule does not run at all, and frames where the FixedUpdate schedule runs multiple times in a row.

To solve this, `lightyear` provides the `VisualInterpolation` plugin.

The plugin will take care of interpolating the position of the entity between the last two `FixedUpdate` ticks, thus making sure that 
the entity is making smooth progress on every frame.

## Three Approaches to Visual Interpolation

### How Lightyear Does it

`lerp(previous_tick_value, current_tick_value, time.overstep_fraction())`

Using the time overstep, it lerps between the current and previous value generated during `FixedUpdate` ticks in accordance with how much time has passed.

The interpolated values are written during `PostUpdate` (see below). The original / canonical value, which was typically set by the physics logic in `FixedUpdate`, is stored, to be written back to the component in `PreUpdate` on the next tick. This means the rendering code should "just work" without being aware interpolation is happening.

**PROS:**
- relatively simple to implement

**CONS:** 
- introduces a visual delay of 1 simulation tick
- need to store the previous and current value (so extra component clones)


### Alternative A

`lerp(current_tick_value, future_tick_value, time.overstep_fraction())`

Simulate an extra step during FixedUpdate to compute the `future_tick_value`, then interpolate between the `current_tick_value` and the `future_tick_value`

**PROS:**
- simulation completely up-to-date, and accurate (if we have inputs for the future tick)

**CONS:** 
- could be less accurate in some cases (inputs didn't arrive in time)
- need to store the previous and current value (so extra component clones)

### Alternative B

Do not interpolate, but instead run the simulation (FixedUpdate schedule) for one 'partial' tick, i.e. we use (time.overstep_fraction() * fixed_timestep) as the timestep for the simulation. 

**PROS:**
- no visual delay
- no need to store copies of the components

**CONS:** 
- we might run many extra simulation steps if we run an extra partial step in every frame


### VisualInterpolationPlugin systems

There are 3 main systems:
- during FixedUpdate, we run `update_visual_interpolation_status` after the `FixedUpdate::Main` set (meaning that the simulation has run).
  In that system we keep track of the value of the component on the current tick and the previous tick
- during PostUpdate, we run `visual_interpolation` which will interpolate the value of the component between the last two tick values
  stored in the `update_visual_interpolation_status` system. We use the `Time<Fixed>::overstep_percentage()` to determine how much to interpolate
  between the two ticks
- during PreUpdate, we run `restore_from_visual_interpolation` to restore the component value to what it was before visual interpolation. This is necessary because
  the interpolated value is not the "real" value of the component for the simulation, it's just a visual representation of the component. We need to restore the real value
  before the simulation runs again.

#### Example
- you have a component that gets incremented by 1.0 at every fixed update step (and starts at 0.0)
- the fixed-update step takes 9ms, and the frame takes 12ms

Frame 0:
- tick is 0, the component is at 0.0
 
Frame 1:
- We run FixedUpdate once in this frame:
    - tick is 1
    - `update_visual_interpolation_status`: set current_value for tick 1 is 1.0, previous_value is None
- `visual_interpolation`: we do not interpolate because we don't have 2 ticks to interpolate between yet. So the component value is 1.0
- the time is at 12ms, the overstep_percentage is 0.33

Frame 2:
- `restore_from_visual_interpolation`: we restore the component to 1.0
- We run FixedUpdate once in this frame:
    - tick is 2
    - `update_visual_interpolation_status`: set current_value for tick 2 is 2.0, previous_value is 1.0
- `visual_interpolation`: the time is 24ms, the overstep percentage is 0.667, we interpolate between 1.0 and 2.0 so the component is now at 1.667

Frame 3:
- `restore_from_visual_interpolation`: we restore the component to 2.0
- We run FixedUpdate twice in this frame:
    - tick is 3
    - `update_visual_interpolation_status`: set current_value for tick 3 is 3.0, previous_value is 2.0
    - tick is 4
    - `update_visual_interpolation_status`: set current_value for tick 4 is 4.0, previous_value is 3.0
- `visual_interpolation`: the time is 36, the overstep percentage is 0.0, we interpolate between 3.0 and 4.0 so the component is now at 3.0
 
Frame 4:
- `restore_from_visual_interpolation`: we restore the component to 4.0
- We run FixedUpdate once in this frame:
    - tick is 5
    - `update_visual_interpolation_status`: set current_value for tick 5 is 5.0, previous_value is 4.0
- `visual_interpolation`: the time is 48, the overstep percentage is 0.33, we interpolate between 5.0 and 4.0 so the component is now at 4.33

So overall the component value progresses by 1.33 every frame, which is what we expect because a frame duration (12ms) is 1.33 times the fixed update duration (9ms).

## Usage

Visual interpolation is currently only available per component, and you need to enable it by adding a plugin:

```rust,noplayground
app.add_plugins(VisualInterpolationPlugin::<Position>::default());
```

You will also need to add the `VisualInterpolateState` component to any entity you want to enable visual interpolation for:
```rust,noplayground
fn spawn_entity(mut commands: Commands) {
    commands.spawn().insert(VisualInterpolateState::<Position>::default());
}
```

## Usage with Avian Physics and FixedUpdate

Here's how you might enable Visual Interpolation for Avian's `Position` and `Rotation`:

```rust,noplayground
app.add_plugins(VisualInterpolationPlugin::<Position>::default());
app.add_plugins(VisualInterpolationPlugin::<Rotation>::default());

app.observe(add_visual_interpolation_components::<Position>);
app.observe(add_visual_interpolation_components::<Rotation>);

// ...

fn add_visual_interpolation_components<T: Component>(
    trigger: Trigger<OnAdd, T>,
    q: Query<&RigidBody, With<T>>,
    mut commands: Commands,
) {
    let Ok(rigid_body) = q.get(trigger.entity()) else {
        return;
    };
    // No need to interp static bodies
    if matches!(rigid_body, RigidBody::Static) {
        return;
    }
    // triggering change detection necessary for SyncPlugin to work
    commands
        .entity(trigger.entity())
        .insert(VisualInterpolateStatus::<T> {
            trigger_change_detection: true,
            ..default()
        });
}
```

If you draw your entities with gizmos based on their `Position` and `Rotation` components, this is all you need.

However, if you have meshes, sprites, or anything that depends on `Transform` (as is the norm), you need to ensure that changes made by the visual interpolation systems are synched to the transforms.

Avian's `SyncPlugin` does this for you, but **beware if you use FixedUpdate**, you need to run the `SyncPlugin` in `PostUpdate`, otherwise you won't be syncing changes correctly. Visual interp happens in `PostUpdate`, even if physics runs in `FixedUpdate`.

```rust,noplayground
// Run physics in FixedUpdate, but run the SyncPlugin in PostUpdate
app
  .add_plugins(
    PhysicsPlugins::new(FixedUpdate)
      .build()
      .disable::<SyncPlugin>(),
  )
  .add_plugins(SyncPlugin::new(PostUpdate));
```

Be aware that moving the `SyncPlugin` to `PostUpdate` could cause issues if you rely on modifying `Transforms` during `FixedUpdate` – they will no longer be synced back to the Avian components during `FixedUpdate`.  If you manipulate physics objects by changing the Avian components directly, it should be fine.

### Avian's SyncConfig

Using Avian's [SyncConfig](https://docs.rs/avian2d/latest/avian2d/sync/struct.SyncConfig.html) you can control how position and transform are synced. You might wish to disable `transform_to_position`, depending on how you game is built.

## Caveats

- The `VisualInterpolationPlugin` is currently only available for components that are present in the protocol. This is because the plugin
  needs to know how interpolation will be performed. The interpolation function is registered on the protocol directly instead of the component
  to circumvent the orphan rule (we want users to be able to define a custom interpolation function for external components)
  - NOTE: This will probably be changed in the future, by letting the user provide a custom interpolation function when creating the plugin.
 
- The interpolation doesn't progress at the same rate at the very beginning, because we wait until there are two ticks available before we start doing the interpolation.
  (you can see in the example above that on the first frame the component is 1.0, and on the second frame 1.667, so the component value didn't progress by 1.33 like it will
  in the other frames).
  - NOTE: it's probably possible to avoid this by just not displaying the component until we have 2 ticks available, but I haven't implemented this yet.

