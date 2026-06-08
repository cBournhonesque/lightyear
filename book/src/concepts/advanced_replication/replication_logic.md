# Replication logic

Lightyear's replication layer is built on `bevy_replicon`.

Replicon tracks registered component changes, serializes them, applies visibility filters, sends server updates, and applies them on the client. Lightyear adds the game-networking pieces around that:

- transport channels for Replicon packets
- server/client connection state
- per-client visibility through `NetworkTarget`, immediate visibility, and rooms
- checkpoint mapping from Replicon ticks to Lightyear simulation ticks
- prediction and interpolation history hooks
- hierarchy helpers

The current supported replication direction is server to client. Clients normally affect replicated state by sending inputs or messages to the server, then the server mutates its world and replicates the result back.

## What gets replicated

Add `Replicate::to_clients(...)` to a server entity to make it part of the replication stream.

Only registered components are sent. Register normal gameplay state with `register_component::<C>()`, and use `register_component_once::<C>()` for components whose value only needs to be sent when the component is inserted or removed.

Replicon handles the usual ECS replication operations:

- entity spawn
- entity despawn
- component insert
- component remove
- component mutation

Those operations are different from a gameplay point of view. A component mutation changes a value on an entity that already exists. A spawn, despawn, insert, or remove can change which systems match the entity. When you design replicated components, keep that in mind: components that must appear together should usually be spawned together, and systems should tolerate the fact that a remote entity only exists after replication has delivered it.

## Server tick and send timing

Replication is emitted when Replicon's server tick advances. Lightyear advances that after the fixed loop has drained, so a frame's fixed simulation changes can be bundled into the outgoing replication work.

For gameplay code, the useful rule is:

- receive network data before simulation
- run fixed simulation
- send network data after simulation

That keeps replicated state aligned with the tick that produced it.

The relevant system sets are:

- `ReplicationSystems::Receive`: applies incoming replication data
- `ReplicationSystems::Send`: flushes outgoing replication data

Most gameplay should not need to schedule directly inside those sets. They are useful when you have glue code that must run before replicated state is applied or before replicated changes are sent.

## Consistency

The practical goal of replication is that the client observes a coherent server state from the past.

For a single entity, that means you should avoid situations where gameplay systems see half of a replicated concept. If `Health` and `Dead` must be interpreted together, register and mutate them with that relationship in mind. If a marker changes which client systems run, make sure the components those systems read are also available when the marker arrives.

For multiple entities, consistency usually comes down to references and hierarchy. If one replicated component points to another entity, the receiver needs a valid local entity mapping before the reference is used. This is why entity mapping is not optional for components that contain `Entity`.

```rust,ignore
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct EquippedBy {
    pub player: Entity,
}

impl MapEntities for EquippedBy {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.player = mapper.get_mapped(self.player);
    }
}
```

The same rule applies to nested structs and collections. If an entity id crosses the network, it has to be mapped.

## Hierarchies

Lightyear has helpers for Bevy relationships such as `ChildOf`.

When a replicated root has children, Lightyear can propagate replication configuration through the hierarchy using `ReplicateLike`. That lets a child use the root's replication target, prediction target, interpolation target, and visibility rules without manually copying the same components everywhere.

Use `DisableReplicateHierarchy` on a child when you do not want that propagation for the child or its descendants.

This is mostly about keeping intent local. You can say "this player entity replicates to these clients" on the root, and the child entities that make up the player can follow that rule.

## Visibility

`Replicate::to_clients(NetworkTarget::...)` is the broad target. Visibility is the fine-grained filter.

Use immediate visibility changes when an entity should be shown or hidden for one client right now. Use rooms when a set of clients and entities share a stable interest-management region.

Visibility also matters for bandwidth. The cheapest replicated update is the one that is not sent.

## Prediction and interpolation hooks

Prediction and interpolation do not replace Replicon. They change how received component data is written when an entity has the relevant marker.

For prediction, incoming server values can be stored as confirmed history so Lightyear can detect mismatches and roll back.

For interpolation, incoming server values can be stored in `ConfirmedHistory<C>` so the interpolation timeline can sample between known states.

Both systems depend on the server stream. They are not client-to-server replication.

## Current limitations

The Replicon backend works best when the server owns replicated state and clients send intent through inputs or messages. A few patterns still need extra care:

- pausing replication by removing `Replicate`
- fully client-authored replicated entities
- some authority-transfer flows

Prespawning is still useful, but treat it as entity matching: the receiving world creates a deterministic entity, gives it a Replicon signature, and lets Replicon match the replicated spawn to that entity later.

When building a game today, prefer the supported path: server-owned replicated entities, client inputs/messages, explicit visibility, and prespawning only where deterministic matching gives you something concrete.
