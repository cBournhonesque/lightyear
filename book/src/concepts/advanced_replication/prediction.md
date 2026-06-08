# Client-side prediction

Client-side prediction is how the local player sees immediate feedback without making the client authoritative.

The server still owns the real state. The client runs a local copy of the same simulation ahead of the latest server update, using the inputs it just produced. When the server's authoritative state arrives, Lightyear can compare it with the predicted history and roll back if they disagree.

## Mental model

- the server replicates one entity
- the client receives a remote entity
- the entity can have a `Predicted` marker
- components registered with `.add_prediction()` keep prediction history
- server updates are written as confirmed history instead of blindly overwriting the predicted value

Prediction is local behavior. It does not mean the client is allowed to replicate entity state back to the server.

## Setup

Register the components that participate in prediction:

```rust,ignore
app.register_component::<PlayerPosition>()
    .add_prediction();
```

When the server spawns the entity, mark which clients should predict it:

```rust,ignore
commands.spawn((
    PlayerBundle::new(client_id),
    Replicate::to_clients(NetworkTarget::All),
    PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
));
```

On the client, run the same deterministic simulation for predicted entities:

```rust,ignore
fn predicted_movement(
    synced: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    mut players: Query<(&mut PlayerPosition, &ActionState<PlayerInput>), With<Predicted>>,
) {
    if synced.is_empty() {
        return;
    }

    for (position, input) in &mut players {
        apply_movement(position, input);
    }
}
```

The server should run the authoritative version of the same movement logic in `FixedUpdate`.

## Rollback

Rollback happens when the client predicted one thing and the server later confirms another.

At a high level:

1. The client receives a server update for an older tick.
2. Lightyear checks the confirmed value against the predicted history for that tick.
3. If they differ, the client restores the confirmed state.
4. The client replays local simulation from that tick to the present.

That is why deterministic fixed-tick systems matter. If the client and server run different logic, prediction will constantly correct itself.

## Which components should be predicted?

Predict the components that are part of simulation state and cannot be cheaply recomputed from other predicted state.

Good candidates:

- position or velocity controlled by input
- physics state that feeds later physics steps
- cooldowns or timers that affect movement or ability logic
- state used by collision or hit detection during prediction

Poor candidates:

- purely visual components
- UI state
- values derived every tick from other predicted components
- server-only authority or persistence markers

When in doubt, start with fewer predicted components. Add more only when a rollback or replay needs that data to be correct.

## Common failure mode

The most common prediction bug is accidentally running different systems on the client and server.

Try to put core movement/combat math in shared functions. Then call those functions from the server authoritative system and the client predicted system. That keeps the behavior close enough that rollbacks are rare and meaningful.
