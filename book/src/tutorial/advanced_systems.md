# Advanced systems

The basic tutorial gives you a server-owned player entity that moves when the server receives client inputs. That works, but it has two visible problems:

- the local player reacts after roughly one round trip
- other players only move when a server update arrives

Prediction and interpolation are the usual fixes.

## Client-side prediction

Prediction lets the owning client run the movement immediately, before the server's update comes back.

Register the component for prediction:

```rust,ignore
app.register_component::<PlayerPosition>()
    .add_prediction();
```

When the server spawns the player, choose which client predicts it:

```rust,ignore
commands.spawn((
    PlayerBundle::new(client_id, Vec2::ZERO),
    Replicate::to_clients(NetworkTarget::All),
    PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
));
```

On the client, run the same movement logic for predicted entities:

```rust,ignore
fn player_movement(
    synced: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    mut players: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
) {
    if synced.is_empty() {
        return;
    }

    for (position, input) in &mut players {
        shared::shared_movement_behaviour(position, input);
    }
}
```

The important part is not the marker. The important part is that client and server run the same deterministic simulation for the same tick. If the client predicts the wrong result, Lightyear can use server updates and prediction history to roll back and replay.

There is no separate "confirmed entity" to draw next to the predicted one in the current model. The server state is kept as history for reconciliation.

## Network interpolation

For entities the local client does not predict, interpolate between server updates.

Register interpolation for the component:

```rust,ignore
app.register_component::<PlayerPosition>()
    .add_linear_interpolation();
```

Then mark the non-owning clients for interpolation:

```rust,ignore
commands.spawn((
    PlayerBundle::new(client_id, Vec2::ZERO),
    Replicate::to_clients(NetworkTarget::All),
    PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
));
```

Interpolation deliberately renders a little behind the latest server update. That gives the client two confirmed states to blend between, which looks much better than snapping to each packet.

## What this gets you

With prediction and interpolation together:

- the local player responds immediately
- remote players move smoothly between server updates
- the server remains authoritative
- clients still send inputs, not replicated entity state

That is the core setup for a server-authoritative action game.
