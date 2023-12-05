# Interest management

Interest management is the concept of only replicating to clients the entities that they need.

For example: in a MMORPG, replicating only the entities that are "close" to the player.


There are two main advantages:
- bandwidth savings: it is pointless to replicate entities that are far away from the player, or that the player cannot interact with.
  Those bandwidth savings become especially important when you have a lot of concurrent connected clients.
- prevent cheating: if you replicate entities that the player is not supposed to see, there is a risk that clients read that data and use it to cheat.
  For example, in a RTS, you can avoid replicating units that are in fog-of-war.



## Implementation

In lightyear, interest management is implemented with the concept of `Rooms`.

An entity can join one or more rooms, and clients can similarly join one or more rooms.

We then compute which entities should be replicated to which clients by looking at which rooms they are both in.

To summarize:
- if a client is in a room but the entity is not (or vice-versa), we will not replicate that entity to that client
- if the client and entity are both in the same room, we will replicate that entity to that client
- if a client leaves a room that the entity is in (or an entity leaves a room that the client is in), we will despawn that entity for that client
- if a client joins a room that the entity is in (or an entity joins a room that the client is in), we will spawn that entity for that client


Since it can be annoying to have always add your entities to the correct rooms, especially if you want to just replicate them to everyone.
We introduce several concepts to make this more convenient.

#### NetworkTarget

```rust,noplayground
/// NetworkTarget indicated which clients should receive some message
pub enum NetworkTarget {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except for one
    AllExcept(ClientId),
    /// Message sent to all clients
    All,
    /// Message sent to only one client
    Only(ClientId),
}
```

NetworkTarget is used to indicate very roughly to which clients a given entity should be replicated.
Note that this is in addition of rooms.

Even if an entity and a client are in the same room, the entity will not be replicated to the client if the NetworkTarget forbids it (for instance, it is not `All` or `Only(client_id)`)

However, if a `NetworkTarget` is `All`, that doesn't necessarily mean that the entity will be replicated to all clients; they still need to be in the same rooms.
There is a setting to change this behaviour, the `ReplicationMode`.


#### ReplicationMode

We also introduce:
```rust,noplayground
#[derive(Default)]
pub enum ReplicationMode {
  /// Use rooms for replication
  Room,
  /// We will replicate this entity to clients using only the [`NetworkTarget`], without caring about rooms
  #[default]
  NetworkTarget
}
```

If the `ReplicationMode` is `Room`, then the `NetworkTarget` is a prerequisite for replication, but not sufficient.
i.e. the entity will be replicated if they are in the same room AND if the `NetworkTarget` allows it.

If the `ReplicationMode` is `NetworkTarget`, then we will only use the value of `replicate.replication_target` without checking rooms at all.