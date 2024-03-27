# Replication Logic

This page explains how replication works and what guarantees can be made.

Replication makes a distinction between:
- Entity Actions (entity spawn/despawn, component insert/remove): these events change the archetype of an entity
- Entity Updates (component update): these events don't change the archetype of an entity but simply update the value of some components. 
    Most (90%+) replication messages should be Entity Updates.

Those two are handled differently by the replication system.

## Invariants

There are certain invariants/guarantees that we wish to maintain with replication.


Rule #1: we would like a replicated entity to be in a consistent state compared to what it was on the server: at no point do we want a situation where
a given component is on tick T1 but another component of the same entity is on tick T2. Similarly, we would not want one component of an entity to be inserted
later than other components. This could be disastrous because some other system could depend on both components being present together!

Rule #2: we want to be able to extend this guarantee to multiple entities.
I will give two relevant examples:
- client prediction: for client-prediction, we want to rollback if a receives server-state doesn't match with the predicted history.
    If we are running client-prediction for multiple entities that are not in the same tick, we could have situations where we need to rollback one entity starting from tick T1
    and another entity starting from tick T2. This can be fairly hard to achieve, so we'd like to have all predicted entities be on the same tick.
- hierarchies: some entities have relationships. For example you could have an entity with a component Head, and an entity Body with a component `HasParent(Entity)`
  which points to the Head entity. If we want to replicate this hierarchy, we need to make sure that the Head entity is replicated before the Body entity.
  (otherwise the `Entity` pointed to in `HasParent` would be invalid on the client). Therefore we need to make sure that all updates for both the parent and the head
  are in sync.


The only way to guarantee that these rules are respected is to send all the updates for a given "replication group" as a single message.
(if we send multiple messages, they could be added to multiple packets, and therefore arrive in a different time/order on the client because of jitter and packet loss)

Lightyear introduces the concept of a [`ReplicationGroup`](crate::prelude::ReplicationGroup) which is a group of entity whose `EntityActions` and `EntityUpdates` will be sent 
over the network as a single message.
It is **guaranteed** that the state of all entities in a given `ReplicationGroup` will be consistent on the client, i.e.
will be equivalent to the state of the group on the server at a given previous tick T.



## Entity Actions

For each [`ReplicationGroup`](crate::prelude::ReplicationGroup), Entity Actions are replicated in an `OrderedReliable` manner:
- we apply each action message *in order*

### Send

Whenever there are any actions for a given [`ReplicationGroup`](crate::prelude::ReplicationGroup), we send them as a single message AND we include any updates for this group as well.
This is to guarantee consistency; if we sent them as 2 separate messages, the packet containing the updates could get lost and we would be in an inconsistent state.

## Entity Updates

### Send

We gather all updates since the most recent of:
- last time we sent some EntityActions for the Replication Group
- last time we got an ACK from the client that the EntityUpdates was received

The reason for this is:
- we could be gathering all the component changes since the last time we sent EntityActions, but then it could be wasteful if the last time we had any entity actions was a long time ago 
  and many components got updated since
- we could be gathering all the component changes since the last time we sent a message, but then we could have a situation where:
  - we send changes for C1 on tick 1
  - we send changes for C2 on tick 2
  - packet for C1 gets lost, and we apply the C2 changes -> the entity is now in an inconsistent state at C2


### Receive


For each [`ReplicationGroup`](crate::prelude::ReplicationGroup), Entity Updates are replicated in a `SequencedUnreliable` manner.
We have some additional constraints:
- we only apply EntityUpdates if we have already applied all the EntityActions for the given [`ReplicationGroup`](crate::prelude::ReplicationGroup) that were sent when the Updates were sent.
  - for example we send A1, U2, A3, U4; we receive U4 first, but we only apply it if we have applied A3, as those are the latest EntityActions sent when U4 was sent
- if we received a more rencet updates that can be applied, we discard the older one (Sequencing)
  - for example if we send A1, U2, U3 and we receive A1 then U3, we discard U2 because it is older than U3
    