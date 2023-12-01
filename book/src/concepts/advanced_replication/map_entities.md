# Mapping Entities


Some messages or components contain references to other Entities.
For example:

```rust,noplayground
#[derive(Message)]
struct SpawnedEntity {
    entity: Entity,
}

#[derive(Component, Message)]
struct Parent {
    entity: Entity,
}
```

In this case, we cannot replicate the Component or Message directly, because the Entity is only valid on the local machine.
So the Entity that the client would receive from the server would only be valid for the Server [`World`](bevy::prelude::World), not the Client's.

We can solve this problem by mapping the server Entity to the corresponding client [`Entity`](bevy::prelude::Entity).

The trait [`EntityMap`](crate::prelude::EntityMap) is used to do this mapping.

```rust,noplayground
pub trait MapEntities {
    /// Map the entities inside the message or component from the remote World to the local World
    fn map_entities(&mut self, entity_map: &EntityMap);
}
```

This is applied to every Message or Component received from the remote World.


Messages or Components implement this trait by default like this:
```rust,noplayground
pub trait MapEntities {
    fn map_entities(&mut self, entity_map: &EntityMap) {}
}
```
i.e. they don't do any mapping.

If your Message or Component needs to perform some kind of mapping, you need to add the `#[message(custom_map)]` attribute,
and then derive the `MapEntities` trait yourself.
```rust,noplayground
#[derive(Message)]
#[message(custom_map)]
struct SpawnedEntity {
    entity: Entity,
}

impl MapEntities for SpawnedEntity {
    fn map_entities(&mut self, entity_map: &EntityMap) {
        self.entity.map_entities(entity_map);
    }
}
```

The [`MapEntities`](crate::prelude::MapEntities) trait is already implemented for [`Entity`](bevy::prelude::Entity).


Note that the [`EntityMap`](crate::prelude::EntityMap) is only present on the client, not on the server; it is currently not possible
for clients to send Messages or Components that contain mapped Entities to the server.


## TODOs

- if we receive a mapped entity but the entity doesn't exist in the client's [`EntityMap`], we currently don't apply any mapping, but still receive the Message or Component.
  - that could be completely invalid, so we should probably not receive the Message or Component at all ?
  - instead we might to wait for the MappingEntity to be created; as soon as it's present in [`EntityMap`] we can then apply the mapping and receive the Message or Component.
    - therefore we need a waitlist of messages that are waiting for the mapped entity to be created

