# Protocol

The protocol is the shared contract between client and server. Both sides need to register the same networked types, in the same app setup, so that messages and replicated components can be serialized and applied correctly.

In practice this usually lives in a shared Bevy plugin.

## Components

Register every component that Replicon is allowed to send.

```rust,ignore
app.register_component::<PlayerId>();

app.register_component::<PlayerPosition>()
    .add_prediction()
    .add_linear_interpolation();

app.register_component_once::<SpawnPoint>();
```

`register_component::<T>()` sends inserts, removes, and later mutations for `T`.

`register_component_once::<T>()` sends inserts and removes, but not later value changes. This is useful for data such as ids, names, or static tags.

The prediction and interpolation calls do not create a separate replication protocol. They add Lightyear's marker/history behavior on top of the Replicon component rule:

- `.add_prediction()` lets authoritative updates for that component be written into prediction history when the entity is predicted.
- `.add_linear_interpolation()` stores confirmed updates in interpolation history and uses linear interpolation between them.
- `.add_custom_interpolation()` stores the history but leaves the interpolation system to you.
- `.register_linear_interpolation()` only registers a lerp function, which is useful for frame interpolation or correction without enabling network interpolation.

There is no separate predicted-entity or interpolated-entity protocol to define. `Predicted` and `Interpolated` are marker components used by the client-side systems.

## Messages

Messages are for typed payloads that are not durable replicated ECS state.

```rust,ignore
app.register_message::<ChatMessage>()
    .add_direction(NetworkDirection::Bidirectional);
```

Use messages for things like chat, loadout choices, menu actions, match requests, or explicit client requests. If a message asks for a world change, the server should validate it and then update replicated state itself.

## Inputs

Inputs are ticked client intent. They are the usual path for player controls in a server-authoritative game.

```rust,ignore
app.add_plugins(input::native::InputPlugin::<PlayerInput>::default());
```

Inputs are handled separately from entity replication because they need timeline behavior: buffering, delay, rebroadcasting, and deterministic processing on the right tick.

## Channels

Channels decide the reliability and ordering used by messages. Replication also registers the Replicon channels it needs internally.

```rust,ignore
app.add_channel::<ReliableChannel>(ChannelSettings {
    mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
    ..default()
})
.add_direction(NetworkDirection::Bidirectional);
```

The usual rule is to pick the weakest guarantee that is correct. Reliable ordered channels are simple, but they are not always the best choice for high-rate gameplay data.
