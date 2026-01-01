# Plan for integration


## Tick

- make LocalTimeline a resource
- will switch Tick to u32 and increment ServerTick manually when it's time to Replicate
  - TODO: replicon needs to give control over when server-tick is incremented!
  - or maybe keep Tick u16? we just increment the RepliconTick by the difference between two ticks. And then

## Prediction

- will relax the assumption that all predicted entities must be on the same tick.
  Instead will start the rollback from the earliest ConfirmedTick from all Predicted entities (similar to Unity)
- maybe make replicon send updates even when nothing changed? (to avoid cases where we mispredict something but there is no correction)

- make Custom replication rule so that replicated components are inserted in PredictionHistory<C> directly.

- enable sending mutation messages every tick (even if empty) and compute ConfirmedTick correctl from ConfirmedHistory and ServerMutateTicks.
- Rewind RepliconTick on rollback


Key insight: `ServerMutateTicks.last_tick = T` guarantees that for entities not updated at tick T,
their value is equal to the last confirmed value.

Proof:
Let's say we have ServerMutateTicks.last_tick = T, and we only received a message for entity A. (there is another entity B).
Does that mean that we fully know the state of entity B? How do we determine the confirmed value for B? We know that the value of B did not change on tick T-1.
- either we received an update for B on tick T-1, then we know that at tick T the value of B is the same
- either we have ServerMutateTicks.T-1 is confirmed, then we know that B at tick T-1 is the same as the previous confirmed value
- either we don't have ServerMutateTicks.T-1 confirmed. We could have:
  - the server did not send any message with an update to B, so B is the same as the previous confirmed value
  - the server sent a message with an update for B, but the message is lost or in-flight. But in that case the server would not have received an ack for that message, so on tick T it would have sent an update for B again! So that is not possible.
That means that we know for sure that B did not change compared to its last confirmed value.

Then the question becomes, how does that affect how we rollback?
We need:
- when we receive an update, we can do a rollback check and add a new confirmed value in the history
- for entities that were not updated, we do a rollback check at ServerMutateTicks.last_tick = T only if the `ServerMutateTick.last_tick` got updated (otherwise we already did the check). When `ServerMutateTick.last_tick` gets updated, then we can set a new confirmed value for all entities that were not updated. This means that the last confirmed value is AT LEAST
- To rollback we have 2 choices:
  - rollback from the earliest confirmed tick across all predicted entities (predicted entities are a subset of all entities so it's possible that this is more recent than ServerMutateTicks.last_tick)
  - rollback from ServerMutateTicks.last_tick
  For simplicity we will do the second choice
- When we rollback:
  - if we do a rollback check, we rollback from the earliest mismatch tick. Subtle: if we receive an update for tick T that mismatches but ServerMutateTicks.last_tick < T (meaning we haven't received all the updates for other ticks), then we can just
  rollback from tick T. The reason is that we can either:
    - rollback from tick T (earliest mismatch)
    - or rollback from earliest confirmed tick X (even among entities that haven't received an update). In which case we would resimulate between ticks X to T but we don't have more recent confirmed values than X, so there's no point in doing that! Instead we rollback from T, but we have our best predicted guess for tick T (even for the entity that didn't receive an update)
  - if we don't do a rollback check, we rollback from ServerMutateTicks.last_tick
- One thing to be careful of is that we could have ServerMutateTicks.last_tick = T, but have received confirmed updates for ticks > T. In which case we don't want to overwrite them when we rollback, and instead use these confirmed values!


## Interpolation

- How would updates be applied?
  - maybe we want to apply updates with history. Imagine we are tick 3 and we receive updates for ticks 5 and 7. Without history, we would only
    get the tick 7 update. But to have better interpolation we want ticks 5 and 7.
  - If `Interpolated` is present, we apply the update to `ConfirmHistory<C>` directly.

## How does replicon work:
- ServerMutateTicks contains the information of 'were all mutation messages received for a given tick'
  - if a tick is confirmed there, then ALL entities are confirmed for AT LEAST this tick
  - some entities might have a more recent confirmed tick, even though all entities for that tick were not confirmed yet, in which case
    ConfirmHistory.last_tick() > ServerMutateTicks.last_tick()
- ConfirmHistory contains any tick where we get an Mutation or Update. (but only if it was 'explicit')
- with MutationTracking, we also send empty packets if there were no mutations for that tick.
  - these can only be seen in ServerMutateTicks; in that case we would have `ServerMutateTicks.last_tick() > ConfirmHistory.last_tick()`
- Mutations for an entity are included in the Update message only for that entity! You can still have an Update message sent together
  with Mutate messages.



## Visibility

- Maintain a mapping from PeerId to NetworkId.
- Make `RemoteId` a visibility filter? or simply `ReplicationSender`?
    - then add a `ClientVisibility` component on each client entity and use that to flip the bit for `ReplicationSender` filter to make
      an entity visible or not depending on the `Replicate` target (and same thing for Predicted/Interpolated)
- ReplicateLike:
  - automatically add the same visibility components as parent entity?

## Protocol

- By default, make `app.register_component` call Replicon's `replicate`
- Also provide an API to start non-networked registration, so that we can customize prediction/interpolation independently from replication.


# Asks
- can we have something in the WriteCtx that tells us if this is from history?
  If it is, there's no need to check for rollback no?

# Prompt

I am working on a networking library using bevy and replicon for replication.
I am designing the prediction system.

Here is how prediction works:
- there are two types of messages: UpdateMessage containing archetype changes (component inserted/removed) and entity spawns/despawns and MutateMessage containing component changes
- All updates are sent together as a single message to guarantee that they are all received on the same tick, but mutations can be sent in multiple messages (so they might not arrive on the same tick, or some messages could even be lost)
- If an entity has both updates and mutations, the mutations are sent as part of the update message.
- Mutations are only applied if all updates prior to the tick when it was sent were also applied
- If there are no mutations on that tick, we still send an empty MutateMessage (to indicate that no component was modified)
- On the receiver side we have 2 ways to track reception:
  - each replicated entity has a ConfirmHistory which indicates the ticks where a mutation/update message was received for that entity
  - there is a global ServerMutateTicks resource that tracks the ticks where ALL messages sent for that tick were received.


The general idea for prediction is:
- each predicted entity has a PredictionHistory<C> for each predicted component. That PredictionHistory would store for each tick an enum with either Predicted(C), Confirmed(C), Removed.
Predicted(C) is the value we are predicting (whenever the value gets modified in the predicted timeline, we add the current value to the history), Confirmed(C) means we received the value from the remote and we know the value is correct.

- we want to rollback to the earliest tick that has any mismatch compared to the prediction history.
For updates that are part of update/mutate messages it is easy:
  - whenever we receive the update, we compare the value with the predicted value stored in the PredictionHistory<C>, if there is a mismatch for that tick T, we should rollback from at least T (maybe earlier if another entity has an even earlier mismatch)
  - when we rollback, we clear all histories and re-run the simulation from the rollback tick to the present tick, to compute a new history.
The main subtlety is for entities that did not change, but that we predicted to change. The general idea is that `ServerMutateTicks` can tell us the ticks where we received ALL sent messages for that tick. In particular that means that entities that did not receive an update were not modified. The question is how can we know the confirmed (i.e. true server value) for these entities?
We could consider that their last confirmed value is the new confirmed one (since the component did not change), but that might not be true. There could be a component value still in flight.
Here is an example:
- we received all messages sent from tick 3 and earlier, so ServerMutateTicks.last_tick = 3
- server sent a mutate for E2 on tick 4, that we haven't received yet
- server sent a mutate for E1 on tick 5, that we just receive. It is the only message sent for tick 5. In that case:
  - we apply the update for tick 5 on E1, which possibly triggers a rollback. The rollback would be from tick 5, which is the earliest tick with any mismatch. When we rollback, we rollback from the
  latest tick where we have received all previous ticks. I.e. the last tick from ServerMutateTicks
  where there are no gaps before that.
  - for E2, we could consider that the confirmed value is the last confirmed value that we have received, which is the one from tick 3. In this case it is not correct since we are about to receive a value for tick 4. I think what we could do is to set a parameter K that is the number of consecutive values we need to have received to consider an unchanged value confirmed. If K = 0, then we can consider the tick 5 value to be confirmed (with the confirmed value being the one previously in the history) and we check for rollback. If K = 1, we need a consecutive value: we need to have received the value on tick 4 before we can consider that the value on tick 5 is confirmed and we can check for rollback.
  - Let's say K=1, and we did a rollback from tick 5, and cleared the histories from tick 3 onwards.
  - Then on a different frame we receive the mutate message for E2 on tick 4:
    - we do a rollback check for E2 and find a misprediction, which triggers a rollback from tick 4
    - we do a rollback check for E1 on tick 4 (since we have received tick 3 and K=1, we can consider tick 4 confirmed)
  - If we receive an empty message on tick 6, then ServerMutateTick.last_tick = 6, and we do a rollback check for E1 and E2.


Some thoughts:
- it could be expensive to do rollback checks. Let's say we want to trigger a rollback every frame, how would we implement it?
  we want to rollback from the last confirmed value. For simplicity that could be ServerMutateTicks.last_tick, or we could use the earliest max(ServerMutateTicks.last_tick, ConfirmHistory.last_tick)
  across all predicted entities.

- what if we want to check for rollbacks?
  we check for mismatch on every message received. For all entities where ServerMutateTick.last_tick > ConfirmHistory.last_tick (meaning that they were not in the latest replication message), we consider that the confirm value is equal to the previous confirmed value in the history (using K. So if K=0, we always do it, if K=1, only if have received a value 1 tick before. If K=2, only if we have received a value 2 ticks before, etc.) and do a mismatch check.
  If there is any mismatch, we rollback from the earliest mismatch and clear the history starting
  from the earliest tick where we have received all ticks. When we rollback, we clear the history from that tick onwards. Maybe we don't clear future values that are confirmed, and when we reach these values we just set the component value to the confirmed one? Or maybe we just clear the whole history and fully re-predict?


- What do you think of this general design? Does it make sense to you, are there any things you would change or improve?
- Do you recommend checking for rollback or always rollbacking? In general, an app has to be performant enough to be able to handle rollbacks, so maybe we might as well always rollback?
- What is your plan for implementation?
