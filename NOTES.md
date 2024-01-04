# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
  - use local executors for async, and use one process/thread per core instead of doing multi-threading (more complicated and less performant
  - one server: 1 game room per core?


- INPUTS:
  - on client side, we have a ActionStateBuffer for rollback, and a ActionDiffBuffer to generate the message we will send to server
  - sometimes frames have no fixed-update, so we have a system that runs on PreUpdate after leafwing that generates inputs as events
    which are only cleared when read
  - then we keep the diffs in a buffer and we send the last 10 or so to the server
  - on the server we reconstruct the action-state from the diff.
  - ISSUES:
    - if we miss one of the diffs (Pressed/Released) because it arrived too late on the client, our action-state on server is on a bad state.
      - I do see cases on server where the current-tick is bigger than the latest action-diff-tick we received.
      - Maybe we should we just send the full action-state every time? (but that's a lot of data)
      - Maybe we could generate a diff even when the action did not change (i.e. Pressed -> Pressed), so that we still have a smaller msesage
        but with no timing information

- Since we have multiple Actionlike, how can we send them?
  - either we add Input1, Input2, etc. in the Protocol
  - either we make the API work like naia with a non-static protocol
    - we maintain a map with NetId of each Channel,Message,Input,Component in the protocol
    - on serialize:
      - we find the netid of the input
      - we'll need to pass the ComponentKinds to the serialize function to get the netid
    - on deserialize:
      - we pass the ComponentKinds to the deserialize function
      - we read the netid and get a `dyn ComponentBuilder`
      - we use the builder to build a `dyn Component`?

 


- CHANGE DETECTION BUG:
  - What are the consequences of this?
    "System 'bevy_ecs::event::event_update_system<lightyear::shared::events::MessageEvent<lightyear::inputs::input_buffer::InputMessage<simple_box::protocol::Inputs>,
    u64>>' has not run for 3258167296 ticks. Changes older than 3258167295 ticks will not be detected."

- SYNC BUGS:
  - still the problem of converting between 2 modulos, which is not possible. Maybe we can ditch WrappedTime and use the actual time, since
    we are never sending it over the network?
  - it looks like sometimes the latest_received_server_tick is always 0, when the sync bug happens
    - that happens because we start at latest_received_server_tick = 0, and we receive a server tick that is >32k, so it's considered 'smaller'
    - FIXED
  - also i've seen a case where client_ideal_tick was lower than server_tick, which is not possible
    - "2023-12-27T23:01:38.614369Z INFO lightyear::client::sync: Finished syncing! buffer_len=7 latency=199.465125ms
      jitter=8.13034ms delta_tick=18160 predicted_server_receive_time=WrappedTime { time: 285.611786s }
      client_ahead_time=40.01602ms client_ideal_time=WrappedTime { time: 285.651802s } client_ideal_tick=Tick(18282)
      server_tick=Some(Tick(18314)) client_current_tick=Tick(122)"
    - it seems that the bug is due to using smoothing on the server_time_estimate. It means that the server_time_estimate can not
      match the server-tick (and be earlier compared to what it would be if we had used the server tick). Temp solution is to
      remove the smoothing on server_time_estimate for now before syncing. (because syncing depends directly on adding a delta)
    - TODO:
      - should i keep it after smoothing? can it have adverse effects?

- BUGS:
  - client-replication: it seems like the updates are getting accumulated for a given entity while the client is not synced
    - that's because we don't do the duplicate check anymore, so we keep adding new updates
    - but our code relies on the assumption that finalize() is called every send_interval (to drain pending-actions/pending-updates) which is not the case anymore.
    - we still want to accumulate updates early though (before client is synced)
    - OPTION 1 (SELECTED):
      - just try sending the updates (which fails because we don't send anything until client is connected). That means 
        we might have a bit of delay to receive the updates at the very beginning (retry_delay).
    - OPTION 2:
      - have a more clever way of accumulating updates. Maybe get a HashMap<ComponentKind, latest-tick> for updates?
      - For actions, we still want to send every update sequentially...
  - input-events are cleared every fixed-udpate, but we might want to use them every frame. What happens if frames are more frequent
    than fixed-update? we double-use an input.. I feel like we should just have inputs every frame?
    Also, only the systems in FixedUpdate get rolled-back, so despawns in Update schedule don't get rolled back. Should we add 
    a schedule in the `Update` schedule that gets rolled-back as well?


- add PredictionGroup and InterpolationGroup?
  - on top of ReplicationGroup?
  - or do we just re-use the replication group id (that usually will have a remote entity id) and use it to see the prediction/interpolation group?
  - then we add the prediction group id on the Confirmed or Predicted components?
  - when we receive a replicated group with ShouldBePredicted, we udpate the replication graph of the prediction group.
- Then we don't really need the Confirmed/Predicted components anymore, we could just have resources on the Prediction or Interpolation plugin
- The resource needs:
  - confirmed<->predicted mapping
  - for a given prediction-group, the dependency graph of the entities (using confirmed entities?)
- The prediction systems will:
  - iterate through the dependency graph of the prediction group
  - for each entity, fetch the confirmed/predicted entity
  - do entity mapping if needed
- users can add their own entities in the prediction group (even if thre )
- examples:
  - a shooter, we shoot bullets. the bullets should be in our prediction group?
    I guess it's not needed if we don't do rollback for those bullets, i.e. we don't give them a Predicted component.
    Or we could create the bullet, make it part of the entity group; the server will create the bullet a bit later.
    When the bullet gets replicated on client; we should be able to map the Confirmed to the predicted bullet; so we don't spawn a new predicted.
    (in practice, for important stuff, we would just wait for the server replication to spawn the entity (instead of spawning it on client and then deleting it if the server version doesn't spawn it?, and for non-important stuff we would just spawn a short-lived entity that is not predicted.)
  - a character has HasWeapon(Entity), weapon has HasParent(Entity) which forms a cycle. I guess instead of creating this graph of deps,
    we should just deal with all spawns first separately! Same for prediction, we first do all spawns first
  
    
- TODO: Give an option for rollback to choose how to perform the rollback!
  - the default option is to snapback instantly to the rollback state.
  - another option is: snapback to rollback state, perform rollback, then tell the user the previous predicted state and the new predicted state.
    for example they could choose to lerp over several frames from the [old to new] (i.e correct only 10% of the way).
    this would cause many consecutive rollback frames, but smoother corrections.
  - register a component RollbackResult<C> {
      // use option because the component could have gotten removed
      old: Option<C>, 
      new: Option<C>,
    }


- DEBUGGING REPLICATION BOX:
  - INTERP TIME is sometimes too late; i.e. we receive updates that are way after interp time.
  - SYNC:
    - seems to not work well for at the beginning..

- FINAL CHOICE FOR REPLICATION:
  - send all actions per group on an reliable unordered channel
    - ordering is done per group, with a sequenced id (1,2,3)
    - thus we cannot receive a despawn before a spawn, or a component removal before a component insert
    - actions are still buffered in advance, so that whenever the latest arrives we apply all of them
    - if there are any actions for an entity, we also send all updates for that entity in the same message!
  - send all updates per group on an unreliable unordered channel
    - sequencing is done per group
    - updates only modify components, do not create them
    - we send all updates since the last SEND system run for which we received an ACK back.
      - so that, if we receive update-17, update-18 and we miss 17, we know that by applying 18 we have a correct world state
      - and we're not missing an update that only happened on 17.
    - we include in the message the latest tick where we sent actions for that entity
    - we do not send an update for a component that has an insert
    - we apply updates immediately for components that exist, and we buffer updates for components/entities that don't exist (this means not in a component)
      - this means that some components (at insert time), can be stuck behind other components; but it's quickly fixed afterwards. All update components are always on the same tick
    - prediction:
      - we store the component history, along with the correct ticks, on the confirmed entity
      - we check rollback on each action or each update for each entity predicted. We rollback on the oldest rollback tick across all predicted entities.
        we restore each predicted entity to their latest update tick.
      - simpler: put all predicted entities in the same group so they have the same tick
      - even simpler: send all updates+actions for predicted entities in the same packet, so updates AND components are all on the same tick.
    - interpolation:
      - we store the component history, along with the correct ticks, on the confirmed entity
  - groups:
    - by default, entities are in their own group GroupId = Entity
    - can specify a group for an entity, and then all entities in that group are in the same group and have their actions / updates sent together
      - useful for hierarchical entities (parent/children)
      - need to make sure parents are sent first in groups I think ? for entity mapping
    - the thing is that groups might be different for each player? not sure if that would ever happen. Not important now
      - rocket league: predict all player entities + ball -> just put all players and ball in the same group
      - normal: predict only the player entity -> no need to put them in the same group.
      - RTS: each player predicts multiple entities -> each player must have their OWN entities in the same group.
        - still fine to put all a player's entities in the same group
  - events:
    - for actions: send an event for each action
    - for updates: send an event for each component update received (buffer)? or applied? applied would be in order, which be better


- interpolation has some lag at the beginning, it looks like the entity isn't moving. Probably because we only got an end but no start?
  - is it because the start history got deleted? or we should interpolate from current to end?
  - the problem is that we get regular update roughly every send_interval when the entity is moving. But when it's not the delay between start and end becomes bigger.
  - when we have start = X, end = None, we should keep pushing start forward at roughly send-interval rate?
   
- interpolation
  - how come the interpolation_tick is not behind the latest_server_tick, even after setting the interpolation_delay to 50ms?
    (server update is 80ms)
    normally it should be fine because we already make sure that interpolation time is behind the latest_server_tick...
    need to look into that.



ADD TESTS FOR TRICKY SCENARIOS:
- replication at the beginning while RTT is 0?
- replication when multiple inserts/removes/updates at same tick
- replication where the data gets split between multiple packets


ROUGH EDGES:
- the bitcode/Bytes parts are confusing and make extra copies
- users cannot specify how they serialize messages/components

- SYNC:
  - sync only works if we send client updates every frame. Otherwise we need to take a LOT more margin on the server
    to make sure that client packets arrive on time. -> MAKE SYNC COMPATIBLE WITH CLIENT UPDATE_INTERVAL (ADD UPDATE_INTERVAL TO MARGIN?)
  - Something probably BREAKS BECAUSE OF THE WRAPPING OF TICK AND WRAPPED-TIME, THINK ABOUT HOW IT WORKS
    - weird wrapping logic in sync manager is probably not correct
  - can have smarter speedup/down for the sync system

TODO:

- Inputs:
  - instead of sending the last 15 inputs, send all inputs until the last acked input message (with a max)
  - also remember to keep around inputs that we might need for prediction!
  - should we store 1 input per frame instead of 1 input per tick? should we enable rollback of Systems in the Update schedule?

- Serialization:
  - have a NetworkMessage macro that all network messages must derive (Input, Message, Component)
    - DONE: all network messages derive Message
  - all types must be Encode/Decode always. If a type is Serialize/Deserialize, then we can convert it to Encode/Decode ?

- Rooms:
  - this was done to limit the size of messages, but paradoxically it might increase the size if the entity doesn't get updated a lot
  - for example, if it's just some background entity, it's better to send them all once, instead of constantly sending them and despawning them
  - for a mmorpg with fixed instances/rooms; is there need to despawn? maybe better if client just despawns anything in the room, and server just stops replicating stuff outside the main room (without despawning though)

- Prediction:
  - TODO: output the rollback output. Instead of snapping the entity to the rollback output, provide the rollback output to the user
    and they can choose themselves how they want to handle it (they could either snap to the rollback output, or lerp from prediction output to rollback output)
   
  - TODO: handle despawns, spawns
      - despawn another entity TODO:
        - we let the user decide 
          - in some cases it's ok to let the created entity despawn
          - in other cases we would like to only despawn that entity if confirm despawns it (for example a common object)
            -> the user should write their systems so that despawns only happen on the confirmed timeline then
    - spawn: TODO
      i.e. we spawn something that depends on the predicted action (like a particle), but actually we rollback,
      which means that we need to kill the spawned entity. 
      - either we kill immediately if it doesn't get spawned during rollback
      - either we let it die naturally; either we fade it out?
      -> same, the user should write their systems so that spawns only happen on the confirmed timeline
      
  - TODO: 2 ways to create predicted entities
    - DONE: server-owned: server creates the confirmed entity, when client receives it, it creates a copy which is a predicted entity -> we have this one
    - TODO: client-owned: client creates the predicted entity. It sends a message to client, which creates the confirmed entity however it wants
      then when client receives the confirmed entity, it just updates the predicted entity to have a full mapping -> WE DONT HAVE THIS ONE YET
     
- Replication:
  - Fix the enable_replication flag, have a better way to enable/disable replication
  - POSSIBLE TODO: send back messages about entity-actions having been received? (we get this for free with reliable channels, but we need to notify the replication manager)

- Message Manager
  - TODO: run more extensive soak test. Soak test with multiple clients, replication, connections/disconnections and different send_intervals?

- Packet Manager:
  - TODO: construct the final Packet from Bytes without using WriteBuffer and ReadBuffer, just concat Bytes to avoid having too many copies

- Channels:
  - TODO: add channel priority with accumulation. Some channels need infinite priority though (such as pings)
    with bandwidth limiting. Should be possible, just take the last 1 second to compute the bandwidth used/left.

- UI:
  - TODO: UI that lets us see which packets are sent at every system update?

- Metrics/Logs:
  - add more metrics
  - think more about log levels. Can we enable sub-level logs via filters? for example enable all prediction logs, etc.

- Reflection: 
  - when can we use this?