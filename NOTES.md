# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
    - use local executors for async, and use one process/thread per core instead of doing multi-threading (more
      complicated and less performant
    - one server: 1 game room per core?

- TODO: create an example related to cheating, where the server can validate inputs


- CROSS-TRANSPORT:
    - TODO:
        - add unit test on metadata
        - maybe replace Channels transport with LocalChannels and multi-connection?

- PRESPAWNING:
    - STATUS:
        - seems to kind of work but not really
        - included the tick in the hash, but maybe we should be more lenient to handle entities created in Update.
        - added rollback to despawn the pre-spawned entities if there is a rollback
        - some prediction edge-cases are handled, but now it bugs when I spawn 2 bullets back-to-back (which means 2
          rollbacks)
    - EDGE CASES TO TEST:
        - what happens if multiple entities have the same hash at the same tick?
            - it should be ok to just match any of them? since we rollback?
            - we could also just in general delete the client pre-spawned entity and then do normal rollback?
        - what happens if we can't match the pre-spawned entity? should then spawn it as normal predicted?
    - TODO
        - simplify the distinction between the 3 predicted spawning types
        - add unit tests
        - handle edge cases of rollback that deletes the pre-spawned entity on the rollback tick.
        - mispredictions at the beginning, why? because of tick snapping?
        - why is there an initial rollback for the matching? it's probably because we only add PredictionHistory at
          PreUpdate
          so we don't have the history for the first tick when the entity was spawned. Might be worth having to avoid
          rollback?
        - the INITIAL ROLLBACK AFTER MATCH SETS THE PLAYER IN A WRONG POSITION, WHY? Because we receive the packet from
          the server
          for the bullet, but not for the player, even though they are in the same replication group!!
        - sometimes the bullet doesn't spawn at all on server, why? input was lost? looks like it was because of a
          tick-snap-event
        - when spawning 2 bullets closely, the second bullet has a weird rollback behaviour
            - that's because on the first rollback, we move all bullets, including the second one that hasn't been
              matched.
            - EITHER:
                - the user makes all systems not run rollback for PreSpawnedPlayerObjects
                - or we rollback PreSpawnedPlayerObjects as well, instead of only entities that have a Confirmed
                  counterpart
            - TODO: maybe add an option for each entity to decide it's rollback eligible?
        - I havea bunch of "could not despawn enttiy because it does not exist", let's check for each entity if it
          exists before despawn?
        - I've seen cases where the bullet is not spawned on the same tick on client and server, why?
        - we rollback the pre-spawned entities all the time because we didn't add a history for them right away..
        - I still frequent rollbacks for the matched entities, weirdly.
        - There are some cases where server/client don't run input on the same tick?
        - Also sometimes we have annoying interpolation freezes..


- SYNC:
    - why is sync breaking after 32700 ticks?
    - if we set the client_tick to something else, then the relationship between time_manager and sync is broken,
      so the timemanager's overstep is not trustworthy anymore?
    - time gets updated during the First system, but i need the time at the end of the frame, so i need to run the
      time-systems myself
      before PostUpdate!
    - we must use the overstep only after FixedUpdate, because that's when it's updated

- PHYSICS:
    - A: if I run FixedUpdate::MAIN AFTER PhysicsSets, I have a smooth physics simulation on client
      if I run FixedUpdate::MAIN BEFORE PhysicsSets, it's very jittery. Why? It should be the opposite!
        - SOLVED: same as C
    - B: the interpolation of the ball is weirdly jittery
        - is it because we run the physics simulation on the interpolated entity?
        - SOLVED: it was because we were running Interpolation at PostUpdate, but drawing at Update.
    - C: collisions cause weird artifacts when we do rollback. Investigate why.
        - check tick 1935. On server, we have some values.
          On client, we have completely different values! After rollback, we get the server values, but only at tick
            1936.
          Is there a off-by-one issue?
          Also the client value was completely different than the server value before the rollback, why?
        - SOLVED: I think I found the issue, it's because we need to run the physics after applying the Actions, but
          before
          we record the ComponentHistory for prediction. This also explains A.
    - D: the client prediction with the ball seems very slighly jittery. Could it be because we
      apply `relative_time_updates` to FixedUpdate on client,
      so we might run a different number of FixedUpdate ticks than on server? Shouln't matter because we still apply
      inputs on the same ticks?
        - HYPOTHESIS:
            1) try interpolating/smoothing the client rollback towards the confirmed output
            2) predict everything!
    - E: client prediction for collisions between two predicted entities (player controlled entities) is also jittery.
      Why?
      Is it because rotation is not replicated?
        - SOLVED: that was it, I needed to replicate angular velocity and rotation as well.
    - F: client is always slightly jittery compared to server.
        - SOLVED: it's because we are predicting 2 entities so we need to make sure that they are in the same
          replication group,
          otherwise the `confirmed_tick` for 1 entity might not be the same as for another entity!
    - G: why does the simulation go completely bananas if there are a lot of mis-predictions? Is it spiral of death?
    - H: having a faster send_interval on the server side makes the simulation less good, paradoxically! Why?
        - do i have an off-by-one error somewhere?
    - I: how to make good client prediction for other clients?
        - maybe we replicate the ActionState of other clients, and then we can do some kind of decay on the inputs; or
          consider
          that they will still be pressed?

STATUS:

- it looks like there's 0 rollback if we only have 1 client and predict the ball. Smooth
- goes crazy if I predict the other player
    - for some reason there was a crazy tick diff. Rollback at tick 934, but client1 was at tick 988!!
    - then the rollback goes crazy because we predict that the entity is moving at their low velocity for the next 50
      ticks.
- I think for proper prediction of other clients we need to know what their latest input was, and then consider that
  they
  pressed the same input during the prediction time. We can also decay the inputs over prediction time?
    - TODO: if we rollback, we ALSO need to restore the other clients inputs to what they were at the rollback tick!!!!
- Status:
    - when I press buttons for client 2, the FPS starts dropping. Death spiral?
    - again, huge tick diff for the rollback (50 ticks) -> WHY???? (only at the start though)
    - client 1 has some mispredictions but overall seems ok?
    - sometimes the rotation is completely different?
    - Not synced during rollback!!!
- STATUS:
    - perfect sync if input_delay > RTT, but with initial sync issues
    - jarring mispredictions if input_delay < RTT
    - sometimes the state is stuck in a misprediction spiral WHY?
    - looks like the input wasn't taken into account???? sometimes the release is not handled correctly.

- STATUS: best-settings. Input-delay = 10, Correction = 8.

- Problem 1: if one of the inputs has a correction, all of them should get have it!
  (after rollback check, iterate through every predicted element in the replication group, and add corrections to those
  who don't have it.
  if they don't have it that means their predicted component is already equal to the confirmed component

  Actually I don't think it's an issue maybe? Because if one of the components doesn't have a correction it's because
  their predicted value was equal to the confirmed value at the time?

- Problem 2: if I set a very high correction-tick number, the movement becomes very saccade for some reason, understand
  that.

- TODO:
    - maybe add an option to disable correction for the player-controlled entities (not entities controlled by other
      players)?
    - why do i need to not sync the ActionState from server to client for it to work? I guess the problem is that the
      ActionState on the Confirmed is ticking and being replicated to Predicted all the time...
    - enable having multiple different rollbacks (with different ticks) for different replication groups
    - i still get stuck inputs even when sending full diffs!!! why?


- TODO: lockstep. All player just see what's happening on the server. that means inputs are not applied on the client (
  no prediction).
  the client just uses the confirmed state from server. (with maybe interpolation)

  CONS: delay.
  PROS: no visual desyncs.

- TODO:
    - DELAYED INPUTS: on local, our render tick (prediction timeline) is 10. But when we press a button, we will add the
      button press to the buffer
      at tick 16, and we immediately send to the server all the actions up to tick 16. The input timeline is in the
      future compared to the render timeline.
      That means that on client, we don't use directly the ActionState (which gets updated immediately), but instead we
      must get the correct ActionState from
      the buffer of ActionStates. i.e. when we reach tick 16, we get the ActionState from the buffer.
    - flow is: press input (at client tick 10), update-action-state, store it in buffer for tick 16, then read from
      buffer at tick 10 to set action set.
        - if the total RTT is smaller than 6 ticks, then we will never get a rollback because server and client will
          have run the same sequence of actions!
    - STATUS:
        - implemented the delayed inputs. The inputs are actually delayed, but it causes some amount of mispredictions,
          even
          with only one client. (which was the opposite of what we wanted lol)
        - implemented quickening the client-time based on delayed inputs, so that there's a lot less prediction to do!
        - with sufficient input delay, I get 0 predictions, past an initial period that doesn't work well.
          For some reason I have some frames where I run a lot of FixedUpdate, and then the client is very desynced.
          It looks like I have 1 frame that took 0.7 seconds ??
    - DEBUG:
        - i'm sending a lot more diffs than necessary. It's because when we fetch the older data from the buffer.
        - i see a case where an action (release Left) has not been received on the server
            - why? maybe just send full diffs then, so that we can recover for this case.
            - absolutely 0 rollbacks with no input delay, so it is related
            - SOLVED: it's because we need to use the delayed action-state at the start of PreUpdate!
        - after that, constant rollbacks, even though the later actions are correct
            - SOLVED: off by 1 error!!!!!!!!!!!!
    - REMAINING ISSUES:
        - in general, at the beginning I do a lot of sync adjustements, and after a while not anymore... why?
        - sometimes at the beginning of sync, I get: "Error too big, snapping prediction time/tick to objective"
          the current-prediction time was way behind the estimated server time
            - TODO: understand this!!!
        - I got a case where immediately after sync, we rollback because the current-tick is further than the
          latest-received-server-tick.
            - SOLVED: don't rollback if the server-tick is ahead of client-tick
        - I got cases where the server-tick on client is ahead of when we should get the input.
            - that's weird because we should have even more margin because we take care of having the client tick ahead
              of server tick...
            - it happens a lot actually!
            - SOLVED: it's because we don't send an input message with no diffs, and we keep popping from the action
              diff buffer, so this log is not reliable.
        - The diffs that the server receives have some duplicate consecutive Pressed. It shouldn't be possible because
          the diffs should only contain
          pure changes
        - with higher tick rate, the frame rate drops to 20.. spiral of death?


- TODO: leafwing inputs should be doing something similar as what I do for replication
    - regularly send full action-state via a reliable channel
    - the rest of the time send unreliable diffs?

- TODO: if something interacts only with the client entity but not on server. The server does not send an update so
  we have no rollback! maybe we should check for rollback everytime the confirmed_tick of the group is updated.

- TODO: also the rollback for a replication group is not done correctly. If ANY component of ANY entity the group is
  rolled back,
  then we should reset ALL components for ALL ENTITIES the group to the rollback state before doing rollback.
  Maybe add a component ConfirmedTick to each replicated entity that has prediction, and do rollback if ConfirmedTick is
  changed.
    - SOLVED: now we reset all components of ALL entities in the group if we need to do rollback.

- TODO: think about handling alt-tabbing on client, which might slow down frames a lot and fuck some stuff?
  Could it be ok for the client to run behind the server, if it just buffers the server's updates?
  i.e. in Receiver we only read the replication messages if the local tick is >= to the remote tick.

- TODO: prediction smoothing. 2 options.
    - we do rollback, compute the new predicted entity now. And then we interpolate by 'smooth' amount to it instead of
      just teleporting to it
      that means that we might do a lot of rollbacks in a row.
    - we do rollback, compute the new predicted entity in X ticks from now. then we enter RollbackStage::Smooth where we
      interpolate
      between now and X+5. We disable checking for rollbacks during that time. Client inputs should affect both the
      entity now and
      the entity we are aiming for (X+5)


- TODO: also remove ShouldBePredicted or ShouldBeInterpolated on server after we have sent it once
- TODO: why do I get duplicate ComponentInsertEvent ShouldBePredicted?
    - SOLVED!
- TODO: when rollback is initiated, only rollback together the entities that have the same replication_group!!!
    - this allows the possibility of having separate replication groups for entities that are predicted but don't need
      to be rolled back together.
- TODO: should the server send other client' inputs to a client so that they can run client-prediction more accurately
  on other clients?
- TODO: physics states might be expensive to network (full f32). What we can do is:
    - compress the physics on the server before running physics step
    - then run physics step
    - and network the compressed physics
    - then the client/server are working with the same numbers, so fewer desyncs
- TODO: input decay: https://www.snapnet.dev/blog/netcode-architectures-part-2-rollback/#input-decay
- TODO: use a sequenced-unreliable channel per actionlike, instead of a global unrealible unordered channel for all
  inputs


- TODO: how to do entity mapping for inputs?
    - A) inputs on pre-predicted entity. Ok because server maintains mapping between server-local and client-predicted!
    - B) inputs on confirmed. Ok because server maintains mapping between server-local and client-confirmed!
    - C) inputs on predicted. How do we do it? Maybe distinguish between Pre-Predicted entities and normal predicted
      entities?
      For normal predicted entities, we map the entity to the confirmed one before sending?

QUESTIONS:

- is there a way to make an object purely a 'collider'? It has velocity/position, but the position isn't recomputed by
  the physics engine
- when does the syncing between transform and rotation/position happen?


- INPUTS:
    - on client side, we have a ActionStateBuffer for rollback, and a ActionDiffBuffer to generate the message we will
      send to server
    - sometimes frames have no fixed-update, so we have a system that runs on PreUpdate after leafwing that generates
      inputs as events
      which are only cleared when read
    - then we keep the diffs in a buffer and we send the last 10 or so to the server
    - on the server we reconstruct the action-state from the diff.
    - ISSUES:
        - if we miss one of the diffs (Pressed/Released) because it arrived too late on the client, our action-state on
          server is on a bad state.
            - I do see cases on server where the current-tick is bigger than the latest action-diff-tick we received.
            - Maybe we should we just send the full action-state every time? (but that's a lot of data)
            - Maybe we could generate a diff even when the action did not change (i.e. Pressed -> Pressed), so that we
              still have a smaller msesage
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
      "System 'bevy_ecs::event::event_update_system<lightyear::shared::events::MessageEvent<lightyear::inputs::
      input_buffer::InputMessage<simple_box::protocol::Inputs>,
      u64>>' has not run for 3258167296 ticks. Changes older than 3258167295 ticks will not be detected."

- SYNC BUGS:
    - still the problem of converting between 2 modulos, which is not possible. Maybe we can ditch WrappedTime and use
      the actual time, since
      we are never sending it over the network?
    - it looks like sometimes the latest_received_server_tick is always 0, when the sync bug happens
        - that happens because we start at latest_received_server_tick = 0, and we receive a server tick that is >32k,
          so it's considered 'smaller'
        - FIXED
    - also i've seen a case where client_ideal_tick was lower than server_tick, which is not possible
        - "2023-12-27T23:01:38.614369Z INFO lightyear::client::sync: Finished syncing! buffer_len=7 latency=199.465125ms
          jitter=8.13034ms delta_tick=18160 predicted_server_receive_time=WrappedTime { time: 285.611786s }
          client_ahead_time=40.01602ms client_ideal_time=WrappedTime { time: 285.651802s } client_ideal_tick=Tick(18282)
          server_tick=Some(Tick(18314)) client_current_tick=Tick(122)"
        - it seems that the bug is due to using smoothing on the server_time_estimate. It means that the
          server_time_estimate can not
          match the server-tick (and be earlier compared to what it would be if we had used the server tick). Temp
          solution is to
          remove the smoothing on server_time_estimate for now before syncing. (because syncing depends directly on
          adding a delta)
        - TODO:
            - should i keep it after smoothing? can it have adverse effects?

- BUGS:
    - client-replication: it seems like the updates are getting accumulated for a given entity while the client is not
      synced
        - that's because we don't do the duplicate check anymore, so we keep adding new updates
        - but our code relies on the assumption that finalize() is called every send_interval (to drain
          pending-actions/pending-updates) which is not the case anymore.
        - we still want to accumulate updates early though (before client is synced)
        - OPTION 1 (SELECTED):
            - just try sending the updates (which fails because we don't send anything until client is connected). That
              means
              we might have a bit of delay to receive the updates at the very beginning (retry_delay).
        - OPTION 2:
            - have a more clever way of accumulating updates. Maybe get a HashMap<ComponentKind, latest-tick> for
              updates?
            - For actions, we still want to send every update sequentially...
    - input-events are cleared every fixed-udpate, but we might want to use them every frame. What happens if frames are
      more frequent
      than fixed-update? we double-use an input.. I feel like we should just have inputs every frame?
      Also, only the systems in FixedUpdate get rolled-back, so despawns in Update schedule don't get rolled back.
      Should we add
      a schedule in the `Update` schedule that gets rolled-back as well?


- add PredictionGroup and InterpolationGroup?
    - on top of ReplicationGroup?
    - or do we just re-use the replication group id (that usually will have a remote entity id) and use it to see the
      prediction/interpolation group?
    - then we add the prediction group id on the Confirmed or Predicted components?
    - when we receive a replicated group with ShouldBePredicted, we udpate the replication graph of the prediction
      group.
- Then we don't really need the Confirmed/Predicted components anymore, we could just have resources on the Prediction
  or Interpolation plugin
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
      When the bullet gets replicated on client; we should be able to map the Confirmed to the predicted bullet; so we
      don't spawn a new predicted.
      (in practice, for important stuff, we would just wait for the server replication to spawn the entity (instead of
      spawning it on client and then deleting it if the server version doesn't spawn it?, and for non-important stuff we
      would just spawn a short-lived entity that is not predicted.)
    - a character has HasWeapon(Entity), weapon has HasParent(Entity) which forms a cycle. I guess instead of creating
      this graph of deps,
      we should just deal with all spawns first separately! Same for prediction, we first do all spawns first


- TODO: Give an option for rollback to choose how to perform the rollback!
    - the default option is to snapback instantly to the rollback state.
    - another option is: snapback to rollback state, perform rollback, then tell the user the previous predicted state
      and the new predicted state.
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
            - so that, if we receive update-17, update-18 and we miss 17, we know that by applying 18 we have a correct
              world state
            - and we're not missing an update that only happened on 17.
        - we include in the message the latest tick where we sent actions for that entity
        - we do not send an update for a component that has an insert
        - we apply updates immediately for components that exist, and we buffer updates for components/entities that
          don't exist (this means not in a component)
            - this means that some components (at insert time), can be stuck behind other components; but it's quickly
              fixed afterwards. All update components are always on the same tick
        - prediction:
            - we store the component history, along with the correct ticks, on the confirmed entity
            - we check rollback on each action or each update for each entity predicted. We rollback on the oldest
              rollback tick across all predicted entities.
              we restore each predicted entity to their latest update tick.
            - simpler: put all predicted entities in the same group so they have the same tick
            - even simpler: send all updates+actions for predicted entities in the same packet, so updates AND
              components are all on the same tick.
        - interpolation:
            - we store the component history, along with the correct ticks, on the confirmed entity
    - groups:
        - by default, entities are in their own group GroupId = Entity
        - can specify a group for an entity, and then all entities in that group are in the same group and have their
          actions / updates sent together
            - useful for hierarchical entities (parent/children)
            - need to make sure parents are sent first in groups I think ? for entity mapping
        - the thing is that groups might be different for each player? not sure if that would ever happen. Not important
          now
            - rocket league: predict all player entities + ball -> just put all players and ball in the same group
            - normal: predict only the player entity -> no need to put them in the same group.
            - RTS: each player predicts multiple entities -> each player must have their OWN entities in the same group.
                - still fine to put all a player's entities in the same group
    - events:
        - for actions: send an event for each action
        - for updates: send an event for each component update received (buffer)? or applied? applied would be in order,
          which be better


- interpolation has some lag at the beginning, it looks like the entity isn't moving. Probably because we only got an
  end but no start?
    - is it because the start history got deleted? or we should interpolate from current to end?
    - the problem is that we get regular update roughly every send_interval when the entity is moving. But when it's not
      the delay between start and end becomes bigger.
    - when we have start = X, end = None, we should keep pushing start forward at roughly send-interval rate?

- interpolation
    - how come the interpolation_tick is not behind the latest_server_tick, even after setting the interpolation_delay
      to 50ms?
      (server update is 80ms)
      normally it should be fine because we already make sure that interpolation time is behind the
      latest_server_tick...
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
      to make sure that client packets arrive on time. -> MAKE SYNC COMPATIBLE WITH CLIENT UPDATE_INTERVAL (ADD
      UPDATE_INTERVAL TO MARGIN?)
    - Something probably BREAKS BECAUSE OF THE WRAPPING OF TICK AND WRAPPED-TIME, THINK ABOUT HOW IT WORKS
        - weird wrapping logic in sync manager is probably not correct
    - can have smarter speedup/down for the sync system

TODO:

- Inputs:
    - instead of sending the last 15 inputs, send all inputs until the last acked input message (with a max)
    - also remember to keep around inputs that we might need for prediction!
    - should we store 1 input per frame instead of 1 input per tick? should we enable rollback of Systems in the Update
      schedule?

- Serialization:
    - have a NetworkMessage macro that all network messages must derive (Input, Message, Component)
        - DONE: all network messages derive Message
    - all types must be Encode/Decode always. If a type is Serialize/Deserialize, then we can convert it to
      Encode/Decode ?

- Rooms:
    - this was done to limit the size of messages, but paradoxically it might increase the size if the entity doesn't
      get updated a lot
    - for example, if it's just some background entity, it's better to send them all once, instead of constantly sending
      them and despawning them
    - for a mmorpg with fixed instances/rooms; is there need to despawn? maybe better if client just despawns anything
      in the room, and server just stops replicating stuff outside the main room (without despawning though)

- Prediction:
    - TODO: output the rollback output. Instead of snapping the entity to the rollback output, provide the rollback
      output to the user
      and they can choose themselves how they want to handle it (they could either snap to the rollback output, or lerp
      from prediction output to rollback output)

    - TODO: handle despawns, spawns
        - despawn another entity TODO:
            - we let the user decide
                - in some cases it's ok to let the created entity despawn
                - in other cases we would like to only despawn that entity if confirm despawns it (for example a common
                  object)
                  -> the user should write their systems so that despawns only happen on the confirmed timeline then
        - spawn: TODO
          i.e. we spawn something that depends on the predicted action (like a particle), but actually we rollback,
          which means that we need to kill the spawned entity.
            - either we kill immediately if it doesn't get spawned during rollback
            - either we let it die naturally; either we fade it out?
              -> same, the user should write their systems so that spawns only happen on the confirmed timeline

    - TODO: 2 ways to create predicted entities
        - DONE: server-owned: server creates the confirmed entity, when client receives it, it creates a copy which is a
          predicted entity -> we have this one
        - TODO: client-owned: client creates the predicted entity. It sends a message to client, which creates the
          confirmed entity however it wants
          then when client receives the confirmed entity, it just updates the predicted entity to have a full mapping ->
          WE DONT HAVE THIS ONE YET

- Replication:
    - Fix the enable_replication flag, have a better way to enable/disable replication
    - POSSIBLE TODO: send back messages about entity-actions having been received? (we get this for free with reliable
      channels, but we need to notify the replication manager)

- Message Manager
    - TODO: run more extensive soak test. Soak test with multiple clients, replication, connections/disconnections and
      different send_intervals?

- Packet Manager:
    - TODO: construct the final Packet from Bytes without using WriteBuffer and ReadBuffer, just concat Bytes to avoid
      having too many copies

- Channels:
    - TODO: add channel priority with accumulation. Some channels need infinite priority though (such as pings)
      with bandwidth limiting. Should be possible, just take the last 1 second to compute the bandwidth used/left.
    - TODO: actually add a priority per ReplicationGroup. The PRIORITY DEPENDS ON THE CLIENT. (for example distance to
      client!)
        - priority
        - update_period: send_interval for this packet. if 0, then we use the server's send_interval.

- UI:
    - TODO: UI that lets us see which packets are sent at every system update?
    - TODO: UI (afte rpriotitization that shows the bandwidth used/priority of each object)

- Metrics/Logs:
    - add more metrics
    - think more about log levels. Can we enable sub-level logs via filters? for example enable all prediction logs,
      etc.

- Reflection:
    - when can we use this?