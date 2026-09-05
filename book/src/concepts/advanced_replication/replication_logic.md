# Replication Logic

This page explains how replication works and what guarantees can be made.

Replication makes a distinction between:
- Entity Actions (entity spawn/despawn, component insert/remove): these events change the archetype of an entity
- Entity Updates (component update): these events don't change the archetype of an entity but simply update the value of some components.
    Most (90%+) replication messages should be Entity Updates.

Those two are handled differently by the replication system.

## Invariants

There are certain invariants/guarantees that we wish to maintain with replication.

**Rule #1**: we would like a replicated entity to be in a consistent state compared to what it was on the server: at no point do we want a situation where
a given component is on tick T1 but another component of the same entity is on tick T2. The replicated entity should be equal to a version of the remote entity in the past.
Similarly, we would not want one component of an entity to be inserted later than other components. This could be disastrous because some other system could depend on both
components being present together!

**Rule #2**: prediction and hierarchies need entities to move in lockstep.
Two relevant examples:
- client prediction: for client-prediction, we want to rollback if a received server-state doesn't match with the predicted history.
    If predicted entities were on different ticks, we'd have to roll each one back from a different tick. Much easier if all predicted entities share the same tick.
- hierarchies: some entities have relationships. For example you could have an entity with a component Head, and an entity Body with a component `HasParent(Entity)`
  which points to the Head entity. If we want to replicate this hierarchy, we need to make sure that the Head entity is replicated before the Body entity.
  (otherwise the `Entity` pointed to in `HasParent` would be invalid on the client).

The way lightyear (via Replicon) honors this is by sending actions and updates for an entity together: whenever there are entity actions to send, the pending updates for the same entities go in the same message. That way a lost packet can't leave you with updates for an entity whose spawn you haven't seen.


## Entity Actions

Entity Actions are replicated reliably and in order.

### Send

Whenever there are actions to send, they go out together with the updates for the same entities.
This is to guarantee consistency; if they went as 2 separate messages, the packet containing the updates could get lost and we would be in an inconsistent state.

### Receive

On the receive side, we buffer the EntityActions that we receive, so that we can read them in order.
Updates are only applied once the actions they depend on have been applied.


## Entity Updates

### Send

We gather all updates since the last time we got an ACK from the receiver that the updates were received.

The reason for this is:
- we could be gathering all the component changes since the last time we sent actions, but then it could be wasteful
if the last time we had any actions was a long time ago and many components got updated since.
- we could be gathering all the component changes since the last time we sent a message, but then we could have a situation where:
  - we send changes for C1 on tick 1
  - we send changes for C2 on tick 2
  - packet for C1 gets lost, and we apply the C2 changes -> the entity is now in an inconsistent state at C2


### Receive

Entity Updates are applied in a sequenced way:
- we only apply updates if we have already applied the EntityActions they were sent with
- if we received a more recent update that can be applied, we discard the older one (sequencing)
  - for example if the server sends U2 then U3 and we receive U3 first, we discard U2 because it is older than U3
