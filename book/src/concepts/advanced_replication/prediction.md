# Client-side Prediction

## Introduction

Client-side prediction means that some entities are on the 'client' timeline instead of the 'server' timeline:
they are updated instantly on the client.

The way that it works in lightyear is that for each replicated entity from the server, the client can choose to spawn 2 entities:
- a Confirmed entity that simply replicates the server updates for that entity
- a Predicted entity that is updated instantly on the client, and is then corrected by the server updates

The main difference between the 2 is that, if you do an action on the client (for example move a character),
the action will be applied instantly on the Predicted entity, but will be applied on the Confirmed entity only after
the server executed the action and replicated the result back to the client.


## Wrong predictions and rollback

Sometimes, the client will predict something, but the server's version won't match what the client has predicted.
For example the client moves their character by 1 unit, but the server doesn't move the character because it detects
that the character was actually stunned by another player at that time and couldn't move.
(the client could not have predicted this because the 'stun' action from the other player hasn't been replicated yet).

In those cases the client will have to perform a rollback.
Let's say the client entity is now at tick T', but the client is only receiving the server update for tick T. (T < T')
Every time the client receives an update for the Confirmed entity at tick T, it will:
- check for each updated component if it matches what the predicted version for tick T was
- if it doesn't, it will restore all the components to the confirmed version at tick T
- then the client will replay all the systems for the predicted entity from tick T to T'



## Edge cases

### Component removal on predicted

Client removes a component on the predicted entity, but the server doesn't remove it.
There should be a rollback and the client should re-add that component on the predicted entity.

Status: added unit test. Need to reconfirm that it works.

### Component removal on confirmed

Server removes a component on the confirmed entity, but the Predicted entity had that component.
There should be a rollback where the component gets removed from the Predicted entity.

Status: added unit test. Need to reconfirm that it works.

### Component added on predicted

The client adds a component on the Predicted entity, but the Confirmed entity doesn't add it.
There should be a rollback and that component gets removed from the Predicted entity.

Status: added unit test. Need to reconfirm that it works.

### Component added on confirmed

The server receives an update where a new component gets added to the Confirmed entity.
If it was not also added on the Predicted entity, there should be a rollback, where the component
gets added to the Confirmed entity.

Status: added unit test. Need to reconfirm that it works.

### Pre-predicted entity gets spawned

See more information in the [client-replication](./client_replication.md#pre-spawned-predicted-entities) section.

Status:
- the pre-predicted entity get spawned. Upon server replication, we re-use it as Predicted entity: no unit tests but tested in an example that it works.
- the pre-predicted entity gets spawned. The server doesn't agree that an entity should be spawned, the pre-spawned entity should get despawned:
  **not handled currently.**

### Confirmed entity gets despawned

We never want to directly modify the Confirmed entity on the client; the Confirmed entity will get despawned only when
the server despawns the entity and the despawn is replicated.

When that happens:
- Then the predicted entity should get despawned as well.
- Pre-predicted entities should still get attached to the confirmed entity on spawn, become Predicted entities and get despawned
  only when the confirmed entity gets despawned.
 
Status: no unit tests but tested in an example that it works.

### Predicted entity gets despawned

There are several options:

OPTION A: Despawn predicted immediately but leave the possibility to rollback and re-spawn it.

We could despawn the predicted entity immediately on the client timeline. If it turns out that the server doesn't despawn
the confirmed entity, we then have to rollback and re-spawn the predicted entity with all its components.
We can achieve this by using the trait
```rust,noplayground
pub trait PredictionCommandsExt {
    fn prediction_despawn<P: Protocol>(&mut self);
}
```
that is implemented for `EntityCommands`.
Instead of actually despawning the entity, we will just remove all the synced components, but keep the entity and the components' histories.
If it turns out that the confirmed entity was not despawned, we can then rollback and re-add all the components for that entity.

The main benefit is that this is very responsive: the entity will get despawned immediately on the client timeline, but 
respawning it (during rollback) can be jarring. This can be improved somewhat by animations: instead of the entity disappearing it can just 
start a death animation. If the death is cancelled, we can simply cancel the animation.


Status:
- predicted despawn, server doesn't despawn, rollback: no unit tests but tested in an example that it works.
  - TODO: this needs to be improved! See note below.
  - NOTE: the way it works now is not perfect. We rely on getting a rollback (where we can see that the confirmed entity
    does not match the fact that the predicted entity was despawned). However we only initiate rollbacks on receiving server updates,
    and it's possible that we are not receiving any updates because the confirmed entity is not changing, or because of packet loss!
    One option would be that `predicted_despawn` sends a message `Re-Replicate(Entity)` to the server, which will answer back by replicating the entity
    again. Let's wait to see how big of an issue this is first.
- predicted despawn, server despawns, we should not rollback but instead despawn both confirmed/predicted when the server
  despawn gets replicated: no unit tests but tested in an example that it works

OPTION B: despawn the confirmed entity and wait for that to be replicated

If we want to avoid the jarring effect of respawning the entity, we can instead wait for the server to confirm the despawn.
In that case, we will just wait for the Confirmed entity to get despawned. When that despawn is propagated, the client entity will
despawned as well.

Status: no unit tests but tested in example.

There is no jarring effect, but the despawn will be delayed by 1 RTT.

OPTION C: despawn predicted immediately and don't allow rollback

If you don't care about rollback and just want to get rid of the Predicted entity, you can just call
`despawn` on it normally.

Status: no unit tests but tested in example.


### Pre-predicted entity gets despawned

Same thing as Predicted entity getting despawned, but this time we are despawning the pre-predicted entity before
we even received the server's confirmation. (this can happen if the entity is spawned and despawned soon after)

Status:
- pre-predicted despawn before we have received the server's replication, server doesn't despawn, rollback:
  - no unit tests but tested in an example that it works
  - TODO: same problem as with normal predicted entities: only works if we get a rollback, which is not guaranteed
- pre-predicted despawn before we have received the server's replication, server despawns, no rollback: 
  - the Predicted entity should visually get despawned (all components removed). When the server entity gets replicated,
    it should start re-using the Predicted entity, initiate a rollback, and see at the end of the rollback that the entity should 
    indeed be despawned.
  - no unit tests but tested in an example that it works
  


