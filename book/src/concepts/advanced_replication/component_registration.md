# Component registration

Replication starts with registration. If a component is not registered, Lightyear will not send it, even if the entity itself is replicated.

That is intentional. A Bevy entity often contains a mix of networked state, server-only bookkeeping, and client-only presentation. Registration is where you decide which parts belong on the wire.

## Normal components

Use `register_component::<T>()` for state that should replicate inserts, removals, and later value changes.

```rust,ignore
app.register_component::<PlayerPosition>();
app.register_component::<Health>();
```

This is the right default for gameplay state that changes over time.

## Insert/remove-only components

Some components need to exist on the client, but their value does not change after spawn. Use `register_component_once::<T>()` for those.

```rust,ignore
app.register_component_once::<PlayerId>();
app.register_component_once::<Team>();
app.register_component_once::<SpawnPoint>();
```

This keeps the entity's shape replicated without paying for mutation updates you do not need.

Good candidates are ids, team assignment, static labels, loadout choices at spawn, and marker-ish data that is not expected to mutate during gameplay.

## Non-networked components

Some components are not sent over the network, but Lightyear still needs to know about them for prediction, interpolation, rollback, or local entity synchronization.

Use `non_networked_component::<T>()` for those.

```rust,ignore
app.non_networked_component::<LocalMoveAccumulator>()
    .add_prediction();
```

This does not register the component with Replicon for network replication. It only creates a `ComponentRegistration` handle so you can attach Lightyear behavior to the component.

## Custom serialization

For most components, `Serialize` and `Deserialize` are enough.

If a type needs special handling, use the `_with` variants and provide your own functions:

```rust,ignore
app.register_component_with::<MyComponent>(
    serialize_my_component,
    deserialize_my_component,
);
```

Use this sparingly. Custom serialization is useful for external types, compact encodings, or compatibility with an existing format, but the simple path is easier to maintain.

## Prediction and interpolation hooks

Prediction and interpolation are added on top of the component registration.

```rust,ignore
app.register_component::<PlayerPosition>()
    .add_prediction()
    .add_linear_interpolation();
```

Those calls do not make the component "more replicated". The component is already replicated by `register_component`. The extra calls tell Lightyear what to do with received server values when the client has marked an entity for prediction or interpolation:

- prediction stores authoritative values in prediction history so rollback can compare and replay
- interpolation stores authoritative values in interpolation history so the client can render between two known server states

You can register both if different clients need different behavior for the same component. The owning client might predict `PlayerPosition`, while other clients interpolate it.

If you only need an interpolation function for frame interpolation or correction, use `register_linear_interpolation()` or `register_interpolation_fn(...)` instead of enabling network interpolation.

## Entity references

If a component stores an `Entity`, it needs entity mapping. Otherwise the client will receive an entity id from the server world and treat it as if it were local, which is wrong.

Use Bevy's `MapEntities` support for those components and register the component in the shared protocol.

```rust,ignore
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct ParentRef(Entity);

impl MapEntities for ParentRef {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.0 = mapper.get_mapped(self.0);
    }
}
```

The rule of thumb is simple: if a value is an entity id from another world, map it before using it.
