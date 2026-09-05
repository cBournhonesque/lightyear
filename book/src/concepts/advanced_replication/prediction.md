# Client-side Prediction

## Introduction

Client-side prediction means that some entities are on the 'client' timeline instead of the 'server' timeline:
they are updated instantly on the client.

The way it works in lightyear: a replicated entity that the client predicts gets a `Predicted` marker on the receiving side. There is only one entity. Its live components hold the predicted values, and each predicted component carries two history buffers on the same entity:

- `ConfirmedHistory<C>`: authoritative states received from the server
- `PredictionHistory<C>`: what the client itself simulated

If you do an action on the client (for example move a character), it applies instantly to the live components. Roughly 1 RTT later the server's authoritative state for that tick arrives in the `ConfirmedHistory`, and the two get compared.

## Wrong predictions and rollback

Sometimes, the client will predict something, but the server's version won't match what the client has predicted.
For example the client moves their character by 1 unit, but the server doesn't move the character because it detects
that the character was actually stunned by another player at that time and couldn't move.
(the client could not have predicted this because the 'stun' action from the other player hasn't been replicated yet).

In those cases the client will have to perform a rollback.
Let's say the client entity is now at tick T', but the client is only receiving the server update for tick T. (T < T')
Every time the client receives an update for tick T, it will:

- check for each updated component if the confirmed state matches what was predicted for tick T
- if it doesn't, it will restore all the components to the confirmed state at tick T
- then the client will replay all the systems for the entity from tick T to T'

## State-based vs input-based rollback

There are two things that can prove a prediction wrong, and they trigger different kinds of rollback:

- **State rollback**: a newly received authoritative state doesn't match the predicted history. This is the case above: the server says "at tick T you were actually here".
- **Input rollback**: a newly received *input* (from another player) doesn't match the input that was assumed when simulating. The client often simulates remote players by repeating their last known input; when the real input arrives and differs, everything simulated with the guessed input has to be re-done from the tick the input changed.

How each kind behaves is controlled by the `RollbackPolicy` on the `PredictionManager`:

```rust,ignore
pub struct RollbackPolicy {
    pub state: RollbackMode,
    pub input: RollbackMode,
    pub max_rollback_ticks: u16, // upper bound on how far back we go (default 20)
}
```

Each mode is one of:
- `Check` (the default): compare against history, only rollback on mismatch.
- `Always`: rollback on every new state/input without comparing. This skips the check cost entirely (for states it also means no `PredictionHistory` needs storing). It's also a good stress test: if your game can't handle the CPU load of constant rollbacks, you'll find out fast.
- `Disabled`: never rollback for that kind. Disable state rollback if you're doing deterministic replication (inputs only); disable input rollback if there are no remote inputs to receive.

If both kinds mismatch at once, state takes precedence: we rollback from the state mismatch.

One caveat of `Always`: your game logic has to *handle* being rolled back at any time. If it can't (e.g. it plays a sound or spawns an entity as a side effect every time it runs), constant rollbacks will expose that immediately. That's the point, but be ready for it.

## Which components should I predict?

You want to predict components for entities that will live in the Predicted timeline, i.e. the timeline that will see changes immediately based on client inputs. However it doesn't mean that every component needs to be actively predicted with `.predict()` (i.e. with rollbacks enabled). A lot of components can be computed from other components and don't need to be predicted or even sent through the network. Here is a quick explanation of which components should be predicted.

As a rule of thumb, a component should be predicted with `.predict()` if it meets the following criteria:

1. Cannot be calculated using the **predicted** components available within the **current** tick.
2. May be modified after creation.

As an example let's look at the `avian3d::position::Position` component provided by the physics simulator `avian`. It describes the position of a 3D object. Its value is modified at the end of each tick by the `avian` physics simulator so it meets the second criteria. If a system wants to know the value of a `Position` component during a given tick, it has no way of calculating that value. Instead, the system will query the `Position` component whose value was calculated in the **previous** tick by the `avian` physics simulator. This means that `Position` also meets the first criteria and so it should be predicted.

A more subtle example is a system that wants to calculate how quickly the length of a ray cast changes. The system would need to know the length of the ray cast in the **previous** tick in order to compare it to the length of the ray cast in then **current** tick. This previous length will have to be stored in a component and that component meets the first criteria as the value it stores cannot be calculated in the **current** tick. The system would then have to save the ray cast's current length in that component after the calculation so that it can be used in the next tick. This is a modification of the component and so it meets the second criteria as well and should be predicted. Here's how the the component and system are defined:
```rust
/// Stores the length of the ray cast from the previous tick.
#[derive(Component)]
struct RayCastPrevLength(f32)

fn calculate_ray_cast_speed(time: Res<Time>, mut query: Query<(&mut RayCastPrevLength, &Position)>) {
  for (mut ray_cast_prev_length, position) in &mut query {
    let curr_ray_cast_length = perform_ray_cast();

    // Perform calculation that relies on information from previous tick.
    let ray_cast_speed = (curr_ray_cast_length - ray_cast_prev_length.0) / time.delta_seconds();

    // Do something with ray cast speed.

    // Save current ray cast length to be used in the next tick.
    ray_cast_prev_length.0 = curr_ray_cast_length;
  }
}
```

If you stored the ray cast speed in a component so that it can be used by other systems then the component does **not** need to be predicted. It is modified every tick so it meets the second criteria, however, it's value is calculated using **predicted** components available in the **current** tick (`RayCastPrevLength`) and so it does not meet the first criteria and does not need to be predicted.

## Edge cases

### Component removal on predicted

Client removes a component on the predicted entity, but the server doesn't remove it.
There should be a rollback and the client should re-add that component on the predicted entity.

Status: added unit test. Need to reconfirm that it works.

### Component removal on confirmed

Server removes a component, but the predicted entity still had it.
There should be a rollback where the component gets removed from the predicted entity.

Status: added unit test. Need to reconfirm that it works.

### Component added on predicted

The client adds a component on the predicted entity, but the server doesn't add it.
There should be a rollback and that component gets removed from the predicted entity.

Status: added unit test. Need to reconfirm that it works.

### Component added on confirmed

The server adds a new component.
If it was not also added on the predicted entity, there should be a rollback, where the component
gets added to the predicted entity.

Status: added unit test. Need to reconfirm that it works.

### Prespawned entity gets matched

See [prespawning](./prespawning.md). When the server entity arrives and matches a prespawned entity, the prespawned entity becomes the predicted entity.

Status:

- the prespawned entity gets matched upon server replication: no unit tests but
  tested in an example that it works.
- the prespawned entity gets spawned but the server never sends a match, the prespawned
  entity should get despawned (timeout cleanup):
  **not handled currently.**

### Server despawns the entity

The client never despawns a replicated entity on its own; the entity gets despawned only when
the server despawns it and the despawn is replicated.

When that happens, the predicted entity gets despawned as well.

Status: no unit tests but tested in an example that it works.

### Predicted entity gets despawned

There are several options:

OPTION A: Despawn predicted immediately but leave the possibility to rollback and re-spawn it.

We could despawn the predicted entity immediately on the client timeline. If it turns out that the server doesn't
despawn
the entity, we then have to rollback and re-spawn the predicted entity with all its components.
We can achieve this by using the trait

```rust,noplayground
pub trait PredictionDespawnCommandsExt {
    fn prediction_despawn(&mut self);
}
```

that is implemented for `EntityCommands`.
Instead of actually despawning the entity, we will just remove all the synced components, but keep the entity and the
components' histories.
If it turns out that the server did not despawn the entity, we can then rollback and re-add all the components for that
entity.

The main benefit is that this is very responsive: the entity will get despawned immediately on the client timeline, but
respawning it (during rollback) can be jarring. This can be improved somewhat by animations: instead of the entity
disappearing it can just
start a death animation. If the death is cancelled, we can simply cancel the animation.

Status:

- predicted despawn, server doesn't despawn, rollback: no unit tests but tested in an example that it works.
    - TODO: this needs to be improved! See note below.
    - NOTE: the way it works now is not perfect. We rely on getting a rollback (where we can see that the confirmed
      entity
      does not match the fact that the predicted entity was despawned). However we only initiate rollbacks on receiving
      server updates,
      and it's possible that we are not receiving any updates because the entity is not changing on the server, or because
      of packet loss!
      One option would be that `predicted_despawn` sends a message `Re-Replicate(Entity)` to the server, which will
      answer back by replicating the entity
      again. Let's wait to see how big of an issue this is first.
- predicted despawn, server despawns, we should not rollback but instead despawn the entity when the
  server
  despawn gets replicated: no unit tests but tested in an example that it works

OPTION B: wait for the server despawn to be replicated

If we want to avoid the jarring effect of respawning the entity, we can instead wait for the server to confirm the
despawn.
In that case, we will just wait for the server despawn to arrive. When that despawn is propagated, the client
entity will
be despawned as well.

Status: no unit tests but tested in example.

There is no jarring effect, but the despawn will be delayed by 1 RTT.

OPTION C: despawn predicted immediately and don't allow rollback

If you don't care about rollback and just want to get rid of the Predicted entity, you can just call
`despawn` on it normally.

Status: no unit tests but tested in example.

### Prespawned entity gets despawned

Same thing as predicted entity getting despawned, but this time we are despawning the prespawned entity before
we even received the server's confirmation. (this can happen if the entity is spawned and despawned soon after)

Status:

- prespawned despawn before we have received the server's replication, server doesn't despawn, rollback:
    - no unit tests but tested in an example that it works
    - TODO: same problem as with normal predicted entities: only works if we get a rollback, which is not guaranteed
- prespawned despawn before we have received the server's replication, server despawns, no rollback:
    - the predicted entity should visually get despawned (all components removed). When the server entity gets
      replicated and matched,
      it should initiate a rollback, and see at the end of the rollback that the
      entity should
      indeed be despawned.
    - no unit tests but tested in an example that it works



