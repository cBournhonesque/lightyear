# Interest management

Interest management is the concept of only replicating to clients the entities that they need.

For example: in a MMORPG, replicating only the entities that are "close" to the player.


There are two main advantages:
- bandwidth savings: it is pointless to replicate entities that are far away from the player, or that the player cannot interact with.
  Those bandwidth savings become especially important when you have a lot of concurrent connected clients.
- prevent cheating: if you replicate entities that the player is not supposed to see, there is a risk that clients read that data and use it to cheat.
  For example, in a RTS, you can avoid replicating units that are in fog-of-war.


## Implementation

### VisibilityMode

The first step is to think about the `VisibilityMode` of your entities. It is defined on the `Replicate` component.

```rust,noplayground
#[derive(Default)]
pub enum VisibilityMode {
  /// We will replicate this entity to all clients that are present in the [`NetworkTarget`] AND use visibility on top of that
  InterestManagement,
  /// We will replicate this entity to all clients that are present in the [`NetworkTarget`]
  #[default]
  All
}
```

If `VisibilityMode::All`, you have a coarse way of doing interest management, which is to use the `replication_target` to 
specify which clients will receive client updates. The `replication_target` is a `NetworkTarget` which is a list of clients 
that we should replicate to.

In some cases, you might want to use `VisibilityMode::InterestManagement`, which is a more fine-grained way of doing interest management.
This adds additional constraints on top of the `replication_target`, we will **never** send updates for a client that is not in the 
`replication_target` of your entity.


### Interest management

If you set `VisibilityMode::InterestManagement`, we will add a `ReplicateVisibility` component to your entity,
which is a cached list of clients that should receive replication updates about this entity.

There are several ways to update the visibility of an entity:
- you can either update the visibility directly with the `VisibilityManager` resource
- we also provide a more static way of updating the visibility with the concept of `Rooms` and the `RoomManager` resource.

#### Immediate visibility update

You can simply directly update the visibility of an entity/client pair with the `VisibilityManager` resource.

```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;

fn my_system(
    mut visibility_manager: ResMut<VisibilityManager>,
) {
    // you can update the visibility like so
    visibility_manager.gain_visibility(ClientId::Netcode(1), Entity::PLACEHOLDER);
    visibility_manager.lose_visibility(ClientId::Netcode(2), Entity::PLACEHOLDER);
}
```

#### Rooms

An entity can join one or more rooms, and clients can similarly join one or more rooms.

We then compute which entities should be replicated to which clients by looking at which rooms they are both in.

To summarize:
- if a client is in a room but the entity is not (or vice-versa), we will not replicate that entity to that client
- if the client and entity are both in the same room, we will replicate that entity to that client
- if a client leaves a room that the entity is in (or an entity leaves a room that the client is in), we will despawn that entity for that client
- if a client joins a room that the entity is in (or an entity joins a room that the client is in), we will spawn that entity for that client

This can be useful for games where you have physical instances of rooms:
- a RPG where you can have different rooms (tavern, cave, city, etc.)
- a server could have multiple lobbies, and each lobby is in its own room
- a map could be divided into a grid of 2D squares, where each square is its own room

```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;

fn room_system(mut manager: ResMut<RoomManager>) {
   // the entity will now be visible to the client
   manager.add_client(ClientId::Netcode(0), RoomId(0));
   manager.add_entity(Entity::PLACEHOLDER, RoomId(0));
}
```