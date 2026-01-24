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

