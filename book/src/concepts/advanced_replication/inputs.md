# Input handling

Inputs are the main way clients affect server-authoritative gameplay.

Lightyear handles inputs separately from ordinary messages because inputs have stronger timing requirements:

- they belong to a specific simulation tick
- the client may need to replay them during rollback
- the server should still receive them if a packet is lost
- other clients may need them for remote-player prediction

The client does not replicate its player entity to the server. It sends intent. The server applies that intent to server-owned state, and the result is replicated back.

## Input backends

Lightyear has a few input integrations. They share the same high-level idea, but they differ in how the local input state is produced.

### Native

The native input plugin is the smallest option. You define an input type, usually an enum or struct, and Lightyear stores it in an `ActionState<I>`.

```rust,ignore
app.add_plugins(input::native::InputPlugin::<PlayerInput>::default());
```

This is a good fit when you want direct control over how keyboard, mouse, gamepad, or automation state becomes a networked input value.

### Leafwing

The Leafwing backend integrates with `leafwing_input_manager`.

```rust,ignore
app.add_plugins(input::leafwing::InputPlugin::<PlayerAction>::default());
```

Use this when your project already uses Leafwing actions, input maps, and action-state ergonomics. Lightyear handles the networking side while Leafwing handles the local input collection.

### BEI

The BEI backend integrates with `bevy_enhanced_input`.

```rust,ignore
app.add_plugins(input::bei::InputPlugin::<PlayerContext>::default());
```

BEI uses input contexts and action entities. Those action entities need to exist on both sides. The current integration uses prespawning-style matching for those entities, because relying on client-to-server entity replication would be the wrong model for the current backend.

## Client system sets

Client input work is split across a few system sets so you can put your own systems in the right place.

The important sets are:

- `InputSystems::ReceiveInputMessages`: receives input messages from other peers, used for rebroadcasted or remote inputs
- `InputSystems::WriteClientInputs`: your system writes the local input for this tick
- `InputSystems::BufferClientInputs`: Lightyear copies the current input into input buffers, or restores an older buffered value during rollback
- `InputSystems::PrepareInputMessage`: builds the message containing this tick's input plus redundant recent inputs
- `InputSystems::RestoreInputs`: restores the action state after preparing delayed input messages
- `InputSystems::UpdateRemoteInputTicks`: updates metadata about remote inputs that have been confirmed
- `InputSystems::SendInputMessage`: sends the prepared input message
- `InputSystems::CleanUp`: drops old buffered values so input buffers do not grow forever

For native inputs, your local input collection usually runs in `FixedPreUpdate`:

```rust,ignore
app.add_systems(
    FixedPreUpdate,
    buffer_input.in_set(InputSystems::WriteClientInputs),
);
```

Leafwing and BEI usually update their action state through their own plugins, so there may be less for you to do in `WriteClientInputs`. You still need to make sure their systems run in a compatible order with Lightyear's input buffering.

## Server system sets

On the server, the important jobs are:

- receive the input message
- write the right tick's input into the server-side input state
- optionally rebroadcast inputs to other clients
- let your fixed simulation read the input

The server-side input sets are:

- `InputSystems::ReceiveInputs`: receive the latest input diffs/messages from clients
- `InputSystems::UpdateActionState`: update the server-side action state for the tick that is about to be simulated

Your gameplay system should run in `FixedUpdate`, after inputs for that tick are available:

```rust,ignore
fn movement(mut players: Query<(&mut PlayerPosition, &ActionState<PlayerInput>)>) {
    for (position, input) in &mut players {
        apply_movement(position, input);
    }
}
```

The exact backend changes the component type you read, but the schedule rule is the same: consume ticked input in fixed simulation.

## Redundancy

Input messages include recent input history, not only the latest value. If packet N is lost, packet N + 1 may still include the input for tick N.

That does not make inputs reliable in the same way as an ordered reliable channel. It is a better fit for real-time controls: if the network loses an old input and the game has moved on, it is usually better to keep going than to stall the simulation waiting for it.

## Rollback

During rollback, Lightyear restores input values from the input buffer for the tick being replayed.

That is why input systems and gameplay systems need to be in fixed schedules. If you read input in arbitrary `Update` systems, rollback cannot replay the same sequence cleanly.

The safest pattern is to put your core movement or combat math in shared functions:

```rust,ignore
pub fn apply_movement(mut position: Mut<PlayerPosition>, input: &PlayerInput) {
    // server authoritative simulation and client prediction both call this
}
```

The server calls it for authority. The client calls it for prediction. If those two paths diverge, prediction corrections become noisy and hard to reason about.
