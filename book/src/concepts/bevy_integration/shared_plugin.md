# Shared plugin

Most Lightyear apps end up with a small shared plugin. It is the place for anything that both peers must agree on:

- replicated component registration
- message registration
- input registration
- deterministic gameplay helpers used by both client and server
- shared bundles, marker components, and simple data types

The exact split is up to your game, but the shared plugin should not contain server-only startup code or client-only rendering code. Keep it boring. If both sides need to serialize a type, map an entity field, or run the same fixed-tick movement function, it probably belongs here.

```rust,ignore
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.register_component::<PlayerId>();
        app.register_component::<PlayerPosition>()
            .add_prediction()
            .add_linear_interpolation();

        app.register_message::<ChatMessage>()
            .add_direction(NetworkDirection::Bidirectional);

        app.add_plugins(input::native::InputPlugin::<PlayerInput>::default());
    }
}
```

Registration has to happen on both sides. Replication is backed by `bevy_replicon`, so client and server must build a compatible component registry before they exchange replicated entities.

For gameplay systems, prefer shared functions over duplicated client and server logic:

```rust,ignore
pub fn apply_movement(mut position: Mut<PlayerPosition>, input: &PlayerInput) {
    // The client can call this for prediction.
    // The server can call the same function for authority.
}
```

That does not mean every system must be shared. Rendering, audio, menus, bots, persistence, and match-making often belong on only one side. The shared plugin is for the small contract that both worlds need to understand.
