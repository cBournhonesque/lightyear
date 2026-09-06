# Interest management

Interest management is the concept of only replicating to clients the entities that they need.

For example: in a MMORPG, replicating only the entities that are "close" to the player.


There are two main advantages:
- bandwidth savings: it is pointless to replicate entities that are far away from the player, or that the player cannot interact with.
  Those bandwidth savings become especially important when you have a lot of concurrent connected clients.
- prevent cheating: if you replicate entities that the player is not supposed to see, there is a risk that clients read that data and use it to cheat.
  For example, in a RTS, you can avoid replicating units that are in fog-of-war.


## Implementation

Visibility is per (entity, client-link) pair: the server only replicates an entity through links it is currently visible to. Visibility is cached, so once you mark an entity visible to a client it stays relevant until you change it again.

`Replicate`'s target composes with visibility as a logical AND: a client outside the target never receives the entity, no matter the visibility. Visibility only narrows things further.

There are two ways to manage it.

### Immediate visibility updates

Use the `VisibilityExt` world methods directly. Here `client` is the link entity (the one with `ReplicationSender`):

```rust,noplayground
world.gain_visibility(entity, client);
world.lose_visibility(entity, client);
```

`lose_visibility` despawns the remote copy. If you'd rather keep the last-known state on the client without further updates, there are two retaining variants:

- `lose_visibility_retained`: only retains the entity if the client has seen it before; otherwise it was never spawned there. Good for last-known-state views or avoiding repeated spawn setup when things move in and out of interest.
- `lose_visibility_always_present`: spawns the entity even while hidden, then pauses updates. Good for roster entries or placeholders that must exist before their live state matters. Don't use it for things that must stay secret, since it reveals existence.

(These map to Replicon's `ScopeLifetime::WhileVisible`, `AfterFirstVisibility` and `AlwaysPresent`.)

### Rooms

For semi-static layouts, rooms are easier than manual per-pair updates. An entity can join one or more rooms, and client links can similarly join one or more rooms. An entity is relevant to a client when they share a room.

This can be useful for games where you have physical instances of rooms:
- a RPG where you can have different rooms (tavern, cave, city, etc.)
- a server could have multiple lobbies, and each lobby is in its own room
- a map could be divided into a grid of 2D squares, where each square is its own room

```rust,noplayground
// setup (once)
app.add_plugins(RoomPlugin);

// allocate rooms and assign them
let room = app.world_mut().resource_mut::<RoomAllocator>().allocate();
commands.spawn((Replicate::to_clients(NetworkTarget::All), Rooms::single(room)));
// ...and put the client link in the same room:
commands.entity(client_link).insert(Rooms::single(room));
```

To summarize:
- if a client is in a room but the entity is not (or vice-versa), we will not replicate that entity to that client
- if the client and entity are both in the same room, we will replicate that entity to that client
- if a client leaves a room that the entity is in (or an entity leaves a room that the client is in), the entity becomes hidden for that client
- if a client joins a room that the entity is in (or an entity joins a room that the client is in), we will spawn that entity for that client

You can see rooms in action in the [network_visibility example](https://github.com/cBournhonesque/lightyear/tree/main/examples/network_visibility).
