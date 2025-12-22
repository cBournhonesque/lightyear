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

- we need the oldest tick that was confirmed among all predicted entities
  - the most recent of `ServerUpdateTick` and `ServerMutateTick.last_tick()` is the OLDEST possible confirmed tick across all entities 
  - it is possible that the Predicted entities are a subset of all replicated entities, in which case we might want to check 
    each Predicted entity's `ConfirmedHistory` to see if the oldest confirmed tick among them is more recent 

- We want to use history for prediction?
  - for cases like:
    - E1 has confirmed tick 7
    - E2 receives ticks 7, 11. Without history we would just apply the tick-11 value, then when we rollback to the earliest confirmed tick
      we would rollback to tick 7 but have an incorrect value.

- New check_rollback logic
  - when we insert a mutation, we do the check rollback then? we compare the new value with the value in the history.


So my current plan for prediction would be:
- I keep track of the `EarliestConfirmedTick`, which is equal to `max(ServerMutateTicks.last_tick, min(ConfirmHistory.last_tick) among all predicted entities)` (the confirmed tick is at least `ServerMutateTick.last_tick`, and it's possible that all predicted entities are at a more recent confirmed tick).
- I check for rollback whenever the predicted component is mutated/removed, in `WriteFn<C>`/`RemoveFn<C>`.
- The value in the prediction history for `EarliestConfirmedTick` is **always confirmed**.
  - either the entity's `ConfirmHistory.last_tick` (meaning that it just received a replication update) is equal to `EarliestConfirmedTick`; this case was handled by `WriteFn<C>`
  - either the entity's `ConfirmHistory.last_tick` is **more recent** than `EarliestConfirmedTick`  in which case the confirmed value was previously inserted in the history by `WriteFn<C>` and we only clear the history up to `EarliestConfirmedTick` 


SITUATION 1
- E1 at tick 7
- E2 at ticks 5, 9, 12. 

Tick 5: server value inserted into history at tick 5. We rollback, there is a new predicted value at tick 7.
Tick 7: we receive an update for E1.
  - either that was the only message for tick 7 and ServerMutateTicks.last_tick = 7, in which case we should rollback starting
    from tick 7. The ConfirmTick for E2 is at tick 5, but ServerMutateTicks.last_tick = 7, which means that the confirmed value is
    the same as the one from tick 5. Restore it from tick 5.
  - either ServerMutateTicks.last_tick < 7 (because we haven't received all messages yet). The ServerMutateTick would for example be 5,
    and we should rollback from tick 5. At tick 7, the component should be inserted on E1. 
    -> WE SHOULD ONLY ERASE THE HISTORY UP TO THE ENTITY'S CONFIRMED TICK, NOT BEFORE!

SITUATION 2
- E1 at tick 7
- we mispredict E1 at tick 9
- We receive an empty ServerMutateTicks.last_tick at tick 9.
  -> we should rollback to the last confirmed tick which was Tick 7.


- For each predicted entity, the latest confirmed tick is `max(ConfirmHistory, ServerMutateTicks)`
 
- PreUpdate:
  - we don't need to check if we received any replication message because:
    - updates received by Predicted entities: we handle it in WriteFn
    - empty mutate messages: these will update ServerMutateTick.last_tick
  - When we receive an update, we update the history and we check for a mismatch. If there is, we set a bool saying that we need to rollback.
  - when we rollback:
    - for each predicted entity, if ConfirmTick < LastConfirmed, then the entity's value didn't change. And we know their confirmed value at LastConfirmedTick! Set their LastConfirmedTick value to the history value of ConfirmTick, and then clear from LastConfirmedTick.
    - if LastConfirmedTick < ConfirmTick, we clear from LastConfirmedTick.
  

// E1 is at tick 4
// E2 is at tick 6
// ServerMutateTick is at tick 2.
// rollback from tick 4 onwards, even though maybe we don't have a confirmed value for tick 4.
// Or should we rollback from tick 2 which is the tick where we know for sure the correct
//  behaviour? (in which case we would have to check if tick 4 was confirmed in E2's ConfirmHistory or not)
// I think it might be better to be optimistic and rollback from tick 4?
// -> YES, cuz even if we rollback from tick 2, we would still have a prediction for 4 (which we have already predicted!) so 

PROBLEM:
- since we rollback from the earliest LastConfirmedTick, then when we rollback and rewrite the history
we lose the confirmed values! Should we separate a ConfirmedHistory from a PredictedHistory?
Inside the PredictedHistory should we have an enum for `Removed, Confirmed, Predicted`? And the Confirmed
value wouldn't be overwritten when we rollback.

- What we could do
-> when we apply all replication updates, we check for corrections. If there are any, we rollback from the earliest correction.
-> whenever ServerMutateTick is updated (we received all updates for that entity), we know that these entities did not change (since the last time the ConfirmedHistory was updated).
  - we optimistically assume that the entity did not change since the last confirmed tick. 

What we can do is:
- start of frame. Reset the EarliestMismatchTick.
- for updates received, we apply them immediately and update the EarliestMismatchTick (which will be where we rollback from). We add the value as Confirmed in the history.
- if ServerMutateTick is updated for that tick (i.e. we receive a MutateTickReceived event), then for all other entities we consider that the value did not change since the last confirmed update. 
  We use the previous confirmed value in the history as the ground truth and consider that it did not change. Then we do a rollback check.
  (in reality, it could have changed in the meantime, but if the replication interval is big compared to frame time, e.g ReplicationInterval = 100ms) there is low chance of that happening.
  - or we only include non-changed updates if the previous RepliconTick was also received?

Edge case:
- ServerMutateTick is T3
- Message with E2-update at T4, still in flight 
- We receive Message with E1-update at T5. It's only message so ServerMutateTick-T5 is confirmed.
  -> rollback check for E1
    - if rollback, rollback from the last mismatch, which is T5.
    - when we rollback, we clear from T3 onwards, T3 is the last ServerMutateTick where ALL previous ticks were confirmed.
  -> no rollback check for E2 because T4 is not confirmed in ServerMutateTick.
      (we only rollback check if the previous tick is also confirmed. We consider that 2 consecutive RepliconTicks should be enough. Make this configurable.
       If we are optimistic we could consider that simply receiving the non-change makes it confirmed since replication interval is big enough.)
    - when we rollback E2, we still start from T5, which is our optimistic prediction.
- We could receive T4 now. If that happens:
  -> rollback check for E2, and add T4 to history
  -> rollback check for E1, we consider that it did not change since T3.
- Then we could receive T6 with no changes.
  -> rollback check for E1 and E2.
  

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

