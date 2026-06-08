# Time sync

Lightyear's replication and inputs are tick-based. A tick is the unit that lets the server say "this happened at simulation step N" and lets the client decide where that update belongs in its local world.

The client tracks more than one timeline:

- `LocalTimeline`: the client's local fixed simulation clock
- `RemoteTimeline`: the client's estimate of the server clock
- `InputTimeline`: the timeline used for local input buffering and prediction

The client does not want to run exactly on top of the server tick. Network packets arrive late and with jitter. Instead, Lightyear keeps estimates and delays that make the client stable enough to process inputs, receive snapshots, and interpolate remote state.

When the client is ready, Lightyear adds `IsSynced<InputTimeline>` and `IsSynced<InterpolationTimeline>` to the client entity. It is common to gate gameplay systems on those markers:

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

The practical rule is simple: fixed simulation belongs in `FixedUpdate`, input collection belongs before it, and rendering can happen later. If a system depends on network time, make the schedule explicit instead of relying on Bevy's default ordering.
