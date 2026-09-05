# Mapping Entities


Some messages or components contain references to other Entities.
For example:

```rust,noplayground
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct SpawnedEntity {
    entity: Entity,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Parent {
    entity: Entity,
}
```

In this case, we cannot replicate the Component or Message directly, because the Entity is only valid on the local machine.
So the Entity that the client would receive from the server would only be valid for the Server [`World`](bevy::prelude::World), not the Client's.

We can solve this problem by mapping the server Entity to the corresponding client [`Entity`](bevy::prelude::Entity).

Bevy's [`MapEntities`](bevy::prelude::MapEntities) trait does this mapping:

```rust,noplayground
pub trait MapEntities {
    /// Map the entities inside the message or component from the remote World to the local World
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M);
}
```

Messages and components implement it as a no-op by default (no mapping). If your type contains entities, implement it yourself:

```rust,noplayground
impl MapEntities for SpawnedEntity {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.entity = entity_mapper.get_mapped(self.entity);
    }
}
```

Then opt the type into mapping at registration. For messages, that's `.add_map_entities()` on the message registration:

```rust,ignore
app.register_message::<SpawnedEntity>()
    .add_direction(NetworkDirection::ServerToClient)
    .add_map_entities();
```

For components, implementing bevy's `MapEntities` is enough; the mapping is applied when the component is received. Without mapping, the inner entities are sent raw and will be meaningless on the other side.

The [`Entity`](bevy::prelude::Entity) type itself already implements `MapEntities`, and so do common containers of entities.


## TODOs

- if we receive a mapped entity but the entity doesn't exist in the client's entity map, we currently don't apply any mapping, but still receive the Message or Component.
  - that could be completely invalid, so we should probably not receive the Message or Component at all ?
  - instead we might to wait for the mapped entity to be created; as soon as it's present in the map we can then apply the mapping and receive the Message or Component.
    - therefore we need a waitlist of messages that are waiting for the mapped entity to be created
