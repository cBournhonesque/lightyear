# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
  - use local executors for async, and use one process/thread per core instead of doing multi-threading (more complicated and less performant
  - one server: 1 game room per core?


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


- CLIENT REPLICATION:
  - we want client to be able to send messages to specific other clients
    - send a message to server, who then retransmits it to other clients
    - either add a MessageWithMetadata in the ProtocolMessage, or add Option<Metadata> in Message.
  - we want client to be able to spawn entities that then become server-authoritative and replicated to other clients
    - for example client spawns snake when it connects.
    - server-authoritative = server updates are replicated to the client (and to other clients)
      - can optionally have a Predicted entity on the client, but in case of conflict server wins.
  - we want client to be able to spawn entities that are client-authoritative and replicated to other clients.
    - for example client's cursor.
    - client-authoritative = client's changes are replicated to the server, which replicates them to other clients than the client.
  - can just use Authority to specify who has authority?
  - maybe we should separate Connection and MessageManager into a Send and Receive part?
    - server recv and client send need MessageWithMetadata
    - client recv and server send need Message
  - on the client prepare spawn, another component gets added, similar to ShouldBePredicted. Or just replicate Replicate entirely, once...
    - 
  - Server authority:
    - OPTION 1
      - client spawns entity, server receives it and stores the mapping client_id=remote_id, server_id=local_id.
      - then server starts replicating back to the client, with a server_id
      - upon reception, the client needs to know that the server_id will map to its own local client_id.
        - i.e. the server's message will contain the client id
        - if the server message contains client-id, instead of spawn we update the local mapping on client
    - OPTION 2:
      - client spawns a predicted entity (Predicted) but Confirmed is nil. id=PC1
      - server receives it and spawns an entity,
  - If jitter is too big, or there is packet loss? it looks like inputs keep getting sent to client 1.
    - the cube goes all the way to the left and exits the screen. There is continuous rollback fails
  - on interest management, we still have this problem where interpolation is stuck at the beginning and doesn't move. Probably 
    because start tick or end tick are not updated correctly in some edge cases.
  - A: client-authoritative replication:
    Client 1 spawns an entity C1, which gets replicated on the client as S1 (and can then be further replicated to other clients).
    S1 doesn't get replicated back to client 1, but only to other clients. For example, we want to replicate client 1's cursor
    position to other clients.
    DONE! Server can just add Replicate when entity is spawned.


  - B: client spawning a Predicted entity.
    For example client 1 spawns a predicted entity P1 (a shared UI menu). Server receives that notification; spawns an
    entity S1 that gets replicated to client 1. Client 1 spawns a confirmed entity C1. But instead of spawning a new
    corresponding predicted entity, it re-uses the existing P1. From there on prediction with rollback can proceed as usual.

    P1 spawns on client. [Sends ShouldBePredicted(predicted=P1) as metadata on the spawn?, just so that the server can send it back]
    Server spawns S1. user adds a system to replicate S1 to other clients (including client 1).
    Client1 spawns C1 (but re-uses C1). [Sends ShouldBePredicted(predicted=P1)]

    - TODO: a client system, upon receiving ShouldBePredicted { client_id: not_none }, attaches Predicted to the entity.
    - TODO: update the confirmed spawn system to handle ShouldBePredicted { not_none }

- OPTION 1:
  - re-use ShouldBePredicted to send the client_entity
  - when we receive back ShouldBePredicted { client_entity } on confirmed.
    - if the current_entity is the client_entity from ShouldBePredicted, ignore (we are the current entity)
    - we use client_entity to find the predicted entity. We add Confirmed and predicted

- OPTION 2: 
  - create with PreSpawned::for_predicted, PreSpawned::for_confirmed. PreSpawned {confirmed: Option, predicted: Option}
  - to start, let's just Prespawned(Entity) because it's easier...
  - use a new component PreSpawned::new() [Prespawned(None)] that the client can add on the entity when it's spawned
  - the server returns PreSpawned(Some(entity)) when it spawns the entity
  - when we receive PreSpawned on client on the confirmed entity:
    - if it's PreSpawned.predicted:
      - if the entity has ShouldBePredicted, we use PreSpawned.predicted to find its predicted entity. We add Confirmed and Predicted.
        - if PreSpawned.predicted is None, we spawn it? maybe not, think about this more... (how to spawn new entities that were despawned.)
          maybe the best solution is to attach Predicted(None) right away to the Predicted entity.
      - if the entity doesnt have ShouldBePredicted, it's an error.
    - if it's PreSpawned.confirmed:
      - when the client receives it; it makes sure not to spawn a new entity, but to re-use the existing one.
      - it's confirmed; so if entity doesn't exist, we spawn it? 


  - C: client spawning a Confirmed entity.
    Client 1 spawns a confirmed entity C1. It gets replicated to server, which spawns S1. Then that entity can get
    replicated to other clients AND to client 1. When client 1 receives the replication of S1, it knows that it corresponds
    to its confirmed entity C1. From there on it's normal replication.

    I'm not actually sure that this is useful; because the entity first spawns instantly on client (i.e. is on the client timeline),
    but then if we fallback to normal replication, the entity then moves to the server's timeline, so there would be a jarring update.



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
  - the sync from confirmed to predict might not only be for replicated components, but also components that were
    spawned on confirmed world directly.
    - which means it's not to apply topological sort during replication; we need to apply it on prediction level directly
    - create a 'PredictionGroup': all predicted entities must be in the same group, and we apply topological sort on that group
      - we actually could have different prediction groups, for entities that we know are not interacting at all!
      - each group has a dependency graph as well
    - maybe maintain a topological sort for each predicted replication group?
      - what about adding new entities to the prediction group? because that's the main problem, otherwise if all the entities
        are known at the beginning we are good!
      - maybe don't need toplogical sort but can just use the vec from the replication to have the order
      - but then how do we iterate through the entities in that order?
    - the components during prediction sync need to be mapped!
    - do we need to introduce the concept of a PredictionGroup, which is a super-set of a replicationGroup? (because some of the entities
        might not come from replication?)
  - how to get smooth interpolation when there can be diagonal movements?
    - allow custom interpolation, so that we can make sure that interpolation respects corners as well. The interpolation follows the path
    - WEIRD: when we just do normal interpolation for player heads, and just use 'interp=start' for tails, it actually looks really smooth!
    - TODO: tried to make my custom interpolation logic working, but there still seems to be edge cases that are not handled well.
      - there's weird panics now, and sometimes the interpolated entity doesn't move at all
  - INTERP TIME is sometimes too late; i.e. we receive updates that are way after interp time.
  - SYNC:
    - seems to not work well for at the beginning..
  - PREDICTION; rollback is weird and fucked
    - looks like sending pings more frequently fixed the issue?, to investigate..
    - is it that we don't receive the inputs on time at the client?
    - imagine we lost some packets and server didn't receive the inputs... then when server receives a later packet it should receive the past 15 inputs.
      server should then use this to rollback?
      - server should ask client to speed up (via a message), when we have lost inputs (to increase the buffer size)
      - it should re-use the previous input as a best guess estimate
      - it looks like our input buffer on server is too big; it contains like 15 ticks worth of inputs, even though the client messages should arrive right before.
        is it just because of the margin we took?
      - applied a best guess estimate for lost inputs that uses the last input sent as fallback, works well!

- FINAL CHOICE:
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

ReplicationManager:
- ActionsManager:
  - for each group, maintain a sequence id for the ordering, and a buffer of actions that are more recent than the sequence_id we are waiting for (waiting-actions)
  - an ActionMessage is GroupId + SequenceId + HashMap<Entity, Spawn or Despawn, Vec<ComponentInserts>, Vec<ComponentRemoves>>
- UpdatesManager:
  - can just use the packet tick for sequencing
  - Maintain a buffer of updates for components/entities that don't exist (waiting-updates), or components that refer to entities that don't exist?
  - an UpdateMessage is GroupId + HashMap<Entity, Vec<ComponentUpdates>>
  - ORDER:
    - recv ReplicationMessage
    - buffer actions
    - read all actions until we get None.
    - apply actions to world
    - flush?
    - read updates. if updates tick is < latest_actions_tick; discard
    - if updates_tick >= latest_actions_tick; apply updates
    - for components that exist (we know them just by checking the world), just apply updates
    - for components that don't exist, buffer updates so that we can apply them as soon as component is created.
- GroupsManager: groups can be tracked as part of Replicate component
- Priority: accumulator for each group? 
- Prediction/Interpolation:
  - probably update component history and check for prediction based on events.

      


- in any case, i can't keep using sequenced reliable/unreliable if i send one message per entity (because then getting packet P2 would prevent me from reading packet P1)

- Several options:
  - one big packet containing ALL actions+updates, in a reliable sequenced channel
    - world is always consistent
  - one big packet containing ALL actions+updates, in a reliable sequenced channel. If no actions, use unreliable sequenced channel.
  - one packet per group actions+updates, in an reliable unordered channel. And we keep track of received tick per group to do sequencing.
    - con: we don't need to retry reliable for the entity updates? but maybe we do if we insist on a consistent state
    - pros: if all the predicted entities are in the same group, no need to use confirmed history for prediction?
  - one reliable unordered packet for group actions, one unreliable unordered packet for group updates
    - apply sequencing manually via a group action tick and group update tick
    - update tick could be ahead of action tick
      (action13: C1 spawn, C3 remove, update14: C1 update, C2 update, C3 update. (C2 already exists) We receive update14 first.
      - option 1: We apply it for all components that exist and set update tick to 14? but then it's not consistent for the components that don't exist)
        C2 is updated to tick 14. When we receive action13, we apply it for C1, and apply the buffered update for C1 so we bring it 14 immediately
        (so that all update ticks are consistent, important for interpolation/prediction)
        -> we still know for each component at which tick we are, but we could be at a state never reached by the server
        -> can't, because some components could depend on actions (for example could reference an entity that doesn't exist)
      - option 2: we apply it for all components, spawning those that don't exist.
        C2 is updated to tick 14, C1 is spawned and set to tick 14.
        -> problem, could get some inconsistency in the entity's archetype. When we receive action13, we don't apply it for C1 (since it exists)
      - option 3: we buffer updates-14, and only apply it when we get action 13 (which could be empty)
        -> in the updates-message, we include the latest action tick for that entity that we send (13).
        -> when we receive updates-14, if we have already received actions-13, then we can apply the updates immediately. If we haven't, we know we need to buffer
        -> it just means we are as slow as the archetype, but that's ok
        -> also if we receive action-13 but update-13 gets lost then it's ok, because they are for different components
           - for each component we still know at which tick we are (important for interpolation/prediction)
        -> so basically we are as far as the latest actions received
        -> on the SEND side, we also have buffers. We buffer updates later than the latest one actions we sent
        -> lets say C1-insert-13, C1-update-14.
      - option 4: on the ticks where there are actions, we send actions+updates reliably!
        on the ticks with no actions, we send updates unreliably.
        In updates, we include the latest action tick we sent.
        On the receive side, we buffer updates, and we apply them only after we received the latest action tick we sent. 
        Example:
        - we send actions-13, updated-17 (with latest actions 13). we receive 17 first, so we buffer it because we haven't received actions-13 yet.
          we receive actions-13, (containing some updates-13 as well), and then apply 17 from the buffer.
          Entity is at 17
        - we send actions-13, updated-17 (with latest actions 13). we receive actions-13 first, so we apply it (entity is at tick 13). Then we receive updates-17, we already received
          the latest actions (13) so we also apply it. The entity is at tick 17.
        - we send actions-13, updates-16(latest-action-13), actions-17, updates-18(latest-action-17)
          - we receive actions-17, we buffer it (ordered reliable)
          - we receive updates-18, we buffer it (cuz we wait for actions-17 to have been applied to client world)
          - we receive updates-16, we buffer it (cuz we wait for actions-13 to have been applied to client world)
          - we receive actions-13, we apply it. we also apply any buffered actions, so actions 17.
          - we flush.
          - we apply updates 16 because actions-13 is reached, and we apply updates-18 cuz actions-17 is reached.
          - (could we do without the flush, and have updates also insert the component ?)
          - entity is at tick 18.
        - we send updates-17 (latest action 12) for C1, updates-18 (latest action 12) for C2. Actions 12 has been received first.
          - we receive update-18 first, we apply it. No need to receive updates-17.
          - that means updates-18 needs to contain ALL CHANGES SINCE ACTIONS-12, not just changes since last sent ?
          - so updates-18 actually contains changes for BOTH C1 and C2
          - or, we just apply updates-18 first, and then when we receive u
        - we could, every 500ms, send all updates as reliable, and between that just send the diff of all components since the reliable state. (delta compression)
            
       
    - actions are applied in order. (if we receive actions 14, we buffer them and wait for actions 13.) 
      How do we make sure of that? For every group, the actions are sent with an id that is incremented in order (1,2,3,4,etc.)
      We wait until we receive the one from the previous id before applying the actions.
      Basically re-implement ordered channel, but manually for this entity/group.
      
    - con: we don't need to retry reliable for the entity updates? but maybe we do if we insist on a consistent state
  - when we receive a server update for tick T, don't apply server updates immediately, but buffer them wait for k * packet_loss + k' * jitter.
    - then we consider that we got the entire consistent world state for tick T, and apply everything. 



ACK SYSTEM: 
- we can receive an ack for a given packet, but systems can't be notified right now if a single given message they sent got received
- 2 problems: can't track acks for unreliable-sender [A], and can't notify other systems [B]
- [A]:
  - create an unordered unreliable sender with ACKs management.
  - includes message-ids, message-acks
- [B]:
  - calling BufferSend returns Option<MessageId> with the id we want to track
  - add a function Channel::follow_acks() -> Receiver<MessageId> that tells us that the message was received.
- SEND message (with notif) -> create a custom id for the notif (re-use message-id for sequenced/reliable senders)
- then store the info in packet-to-message-ack. Maybe MessageAck contains ack-id instead of message-id; or store in dedicated AckId.
- we update packet-to-message-acks if: channel is reliable (message-id is set)
- when we receive, we remove the bundle of message-acks from packet-to-message-ack for the packet we just got
- for each message-ack, we send a 'ACK' via a crossbeam channel?

NEW REPLICATION APPROACH:
- priority:
  - accumulate priority score per entity (or group)
- replication:
  - maybe send the entire actions+updates as one sequenced reliable message?
  - or, if there are no actions this tick, send as sequenced unreliable?
- rooms:
  - this was done to limit the size of messages, but paradoxically it might increase the size if the entity doesn't get updated a lot
  - for example, if it's just some background entity, it's better to send them all once, instead of constantly sending them and despawning them
  - for a mmorpg with fixed instances/rooms; is there need to despawn? maybe better if client just despawns anything in the room, and server just stops replicating stuff outside the main room (without despawning though)
- entity actions are sent as a single message, so that the archetype world state is always consistent
  - or do that only within groups! (so entities in a given group are always consistent)
  - groups could use priority with accumulation to do throttling.
- updates are sent as one group of update per entity.
- for an entity, we can track:
  - it's actions-tick (tick at which the server entity actions were sent)
  - it's updates-tick (tick at which the server entity updates were sent)



Some scenarios:
- we send E-spawn on tick 13, E-despawn on tick 14. E-despawn arrives first.
  - then we update our internal state to have E: action-tick = 14, so we ignore the tick-13 spawn -> GOOD
  - TODO: this means we need to keep track of the action-tick/updates-tick OUTSIDE of a component, since we need to update it even if the component does not exist
  - we keep track of an entity's replication state for at least time to handle de-sync like this:
    - k * send_interval + k' * jitter (k' = 3 for 99% jitter, k = 2 to handle 1 packet lost)
- we send C-insert on tick 13, C-remove on tick 14, C-insert on tick 15. We receive 14, 15, 13.
  - we update our internal state to have C: remove-tick = 14, so we ignore the tick-13 insert -> GOOD
  - we update our internal state to have C: updates-tick = 15, so we add the component -> GOOD
- we send C-insert on tick 13, C-update on tick 14. We receive 14, 13
  - TODO: don't send insert and update on the same tick, only send insert!
  - EITHER:
    - we spawn C with value of 14. And then we can ignore insert. But then the world state in terms of archetypes could again be incorrect?
    - we buffer C as a pending update. Then later when we receive 13, we spawn C with value of 13, and immediately apply the update
      Action tick = 13, update tick = 14.
      TODO: need a buffer of component updates along with their tick.
      - we might need it either way for prediction/interpolation
    
- Update interpolation history:
  - stop using latest_recv_server_tick to put stuff in the history, instead use the entity's update-tick
- Prediction
  - let's say that all the entities that are predicted at the same time are in the same group. Then their world is consistent
  - we know they all receive an update at the same tick (entity update tick)
  - so whenever we receive a new server packet for any entities in the group, we know that all the entities are consistently on tick T
  - we check if need to rollback for tick T
  - if yes, we rollback from tick T, which is easy (no need to have confirmed histories)
   

PROBLEMS/BUGS:
- Big problem with sequencing. Right now we use a single channel for all entity updates.
  - but imagine we send [A1, B2, C3] in packet 1, and [B4] in packet 2 (the numbers are the message ids, incremented in the SequencedSender)
  - then we receive [B4] before the other packet. That means that because of sequencing we ignore [A1, B2, C3].
  - ignoring B2 is good because we received a more recent update for B, but we should not ignore A1 and C3.
  - that means we should have separate sequencing guarantees for each entity/component?
  - in our case, remember that all updates for one entity are in the same message. But we could have B be the updates for entity B and A for entity A.
    then we would completely not receive the updates for entity A.
  - Instead we can:
    - use unordered unreliable channel
    - keep track of the latest tick received per entity update
  - REMEMBER THAT THOSE PROBLEMS ARISE ONLY IF WE HAVE MULTIPLE PACKETS, MIGHT BE REALLY RATE?
    - can happen if lots of replication stuff to send


- Other sequencing problem:
  - let's say that we send [E-A-update] in packet 1, [E-A-removal] in packet 2 and packet 2 arrives before packet 1
    - can only happen if jitter is big compared with send_interval
    - within a single frame, we won't send both an update and a removal for the same entity/component
  - then removal of component A for entity E gets applied, but then we receive update, which re-inserts the entity!
  - TODO: Maybe updates should just update the entity and not re-insert it!

- TLDR: Basically lots of bugs if jitter is big compared with send_interval! 
  (i.e. if some packets)
  



- more rollbacks than expected (Especially with rooms). Let's check what happens with 0 jitter -> there should be 0 rollback no?
  - is it because we are sending too much data to the client? (a lot of entity spawn/despawn)
  - is it because the ticks are not in sync?
  - with 0 jitter, the problem completely disappears, prediction becomes butter smooth
  - with higher send_interval, the prediction becomes extremely jittery!!!
  - It could be something like:
    - we are sending the player position update for a tick 18 in a packet
      is only received after. So if we first receive a PING for tick 20, we think we have the world state for tick 20, but we don't. We don't even have the world state for tick 18 for that one entity.
    - worst part is that we do a wrong rollback for at tick! We do a rollback for tick 20, but then we don't do it when we later receive the update for tick 18, because it's not a latest_recv_server_tick

  - this is exacerbated for entity actions because they are sent on a reliable channel.
    the packet tick may say tick 300, but the entity insert was actually done only at tick 290. (because of packet loss, the message is sent again on a different packet)
    Should entity actions include the tick at which they were actually inserted?
    Actually this is for any replication message that is sent on a reliable channel.

  - SOL 1:
    - keep checkinf for rollback only when we receive a new latest_received_server_tick.
    - when receive a packet at tick T and we have last_received_server_tick = T, we know that we should do a rollback check after 2 * k * jitter has elapsed (jitter could be in both directions).
      because at this time all the packets for tick T have been received. At that time, the confirmed state is for tick T.
      This might not work if the server send_interval is small compared to jitter.
      Thus we only start checking for rollback at (server_latest_recv_time + 2 * k * jitter) (and that's what we should do anyway,
      because only then do we know that we have a full world state)
    - PROS: more elegant?
    - CONS: 
      - seems to not work well if send_interval is very small, because the server state at server_ltaest_recv_time + 2 * k * jitter won't be the server state for tick T
        but for a tick T + x, as we we will have received other updates
 
 
  - SOL 2:
    - when we receive an update for an entity, keep track of the server_tick for that update and add it to a ConfirmedHistory for that tick.
      - maybe keep track of the tick for each component?
      - maybe keep track of the tick for updates / removes / inserts?
      - maybe keep track of the tick for the entity?
    - then we do a rollback check for that entity. If there is mismatch, we need to do a rollback for at least tick T (tick of the packet that contained the entity update)
    - we do this across all entities for which we receive an update, and we compute that the earliest rollback we need to do. (earliest tick across all entities) T*
    - we can do the rollback because:
      - we have the history of all confirmed components since T*
        - we need to keep histories for at most 2*k*jitter + packet-loss ?
      - we have client inputs since T* (we cannot do the rollback if we don't have client inputs since T*)
    - note that we have a similar problem for interpolation:
      - we could send C1 at tick 18, C2 at tick 20. (happens if we send updates quickly)
      - but C2 is received first because of jitter, so latest_recv_server_tick is set to 20.
      - we then add C2 in history with tick 20 because it changes.
      - we receive C1 later, we add C1 in history for tick 20 (which is incorrect because it should be for tick 18)
      - in general, knowing the exact tick of the update is valuable




  - also it seems like our frames are smaller than our fixed update. This could cause issues?
  - CONFIRMATION:
    - the problem is indeed that we receive a ping for tick 20, but the world state for tick 20 is only received after
      so the rollback thinks there is a problem (mismatch at tick 20 + rolls-back to a faulty state 20)
    - it was more prevalent with send_interval = 0 because the frame_duration was lower than tick_duration. So sometimes
      we would have: frame1/tick20: send updates. frame2/tick20: send ping. And the ping would arrive before.
    - BASICALLY, when we receive a packet for tick 10, we think the whole word is replicated at tick 10, but that's not the case.
      Only the entities in that packet. TO fix this:
      - try hard to put all messages for a single entity in the same packet. Even though they might have different channels
      - if we can't, let's care mostly about the latest tick for the entity-updates.
      - for rollback, we track for each entity the latest tick for which we have received an update.
      - during rollback check, we check if there is a mismatch for each entity (and we take the latest entity-update for that entity)
      - if there is a mismatch, we 
  
  - i think the reason is this:
    - we send multiple packets for tick 20
    - some of them (entity spawn, etc.) arrive at tick 25 on client. latest_serv_tick = 20
    - we consider that the world update for tick 20 is received!
    - some of them (player position) arrive later on client.

- room management:
  - when moving fast, some entities don't get despawned on the client
    - it's probably because the spawn message (from joining a room) arrives after the despawn message


- interpolation has some lag at the beginning, it looks like the entity isn't moving. Probably because we only got an end but no start?
  - is it because the start history got deleted? or we should interpolate from current to end?
  - the problem is that we get regular update roughly every send_interval when the entity is moving. But when it's not the delay between start and end becomes bigger.
  - when we have start = X, end = None, we should keep pushing start forward at roughly send-interval rate?
   
- interpolation
  - how come the interpolation_tick is not behind the latest_server_tick, even after setting the interpolation_delay to 50ms?
    (server update is 80ms)
    normally it should be fine because we already make sure that interpolation time is behind the latest_server_tick...
    need to look into that.

- interpolation is very unsmooth when the server update is small.  
   - SOLVED: That's because we used interpolation delay = ratio, and the send_interval was 0.0
   - we need a setting that is ratio with min-delay

- map entities is not working


ADD TESTS FOR TRICKY SCENARIOS:
- replication at the beginning while RTT is 0?
- replication when multiple inserts/removes/updates at same tick
- replication where the data gets split between multiple packets


ROUGH EDGES:
- users cannot derive traits on ComponentProtocol or MessageProtocol because we add some extra variants to those enums
- the bitcode/Bytes parts are confusing and make extra copies
- users cannot specify how they serialize messages/components

- SYNC:
  - sync only works if we send client updates every frame. Otherwise we need to take a LOT more margin on the server
    to make sure that client packets arrive on time. -> MAKE SYNC COMPATIBLE WITH CLIENT UPDATE_INTERVAL (ADD UPDATE_INTERVAL TO MARGIN?)
  - Something probably BREAKS BECAUSE OF THE WRAPPING OF TICK AND WRAPPED-TIME, THINK ABOUT HOW IT WORKS
    - weird wrapping logic in sync manager is probably not correct
  - can have smarter speedup/down for the sync system

- MapEntities:
  - if we receive a mapped entity but the entity doesn't exist, we just don't do any mapping; but then the entity could be completely wrong?
    - in that case should we just wait for the entity to be created or present in the mapping (this is what naia does)? And if it doesn't get created we just ignore the message?
    - the entity mapping is present in the entity_map which exists on client, but not on server. So we cannot do the mapping on server.



TODO:

- Inputs:
  - instead of sending the last 15 inputs, send all inputs until the last acked input message (with a max)
  - also remember to keep around inputs that we might need for prediction!

- Serialization:
  - have a NetworkMessage macro that all network messages must derive (Input, Message, Component)
    - DONE: all network messages derive Message
  - all types must be Encode/Decode always. If a type is Serialize/Deserialize, then we can convert it to Encode/Decode ?

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

- UI:
  - TODO: UI that lets us see which packets are sent at every system update?

- Metrics/Logs:
  - add more metrics
  - think more about log levels. Can we enable sub-level logs via filters? for example enable all prediction logs, etc.

- Reflection: 
  - when can we use this?




# Interest Management

TODO: What is the benefit of doing this room thing instead of just letting the user set
replication_target = "Select(hashSet<ClientId>)" ?

- Clients can belong to multiple rooms, rooms can contain multiple clients
  - we have a Map<ClientId, Hashset<RoomId>>
  - we have a Map<RoomId, Hashset<ClientId>>
- each Entity belongs to a room. If the client also belongs to that room, we replicate.
  - if entity has no room, replicate to no-one
  - if entity is in special-room All, replicate to everyone
  - MAYBE: if entity is in special-room Only(id), replicate only to that client
  - MAYBE: if entity is in special-room Except(id), replicate to everyone except that client
  - each entity keeps a cache of the clients (HashSet<ClientId>) they are replicating too -> current status of which clients the entity should be replicating too
    - everytime there is an event (ClientConnect, ClientDisconnect, ClientLeaveRoom, ClientEnterRoom,), we update all caches
      - for every entity; check if that client id is in the same room (if it is, add it to the cache; if it's not )
      - if the cache for that entity is updated, emit a ClientGainedVisibility or ClientLostVisibility
    - if the entity leaves/enters any room, we update the cache as well.
      - we cannot directly update the cache upon leave/enter, because other clients might be joining/exiting the room at the same time
    - EntityLeaveRoom/EntityEnterRoom/ClientLeaveRoom/ClientEnterRoom should happen during Update.Main, and cache update will happen
      on some PostUpdate system-set
    - we will use a resource to keep track of all the pending room changes. We recompute the caches only for:
      - entities that have a EntityLeaveRoom/EntityEnterRoom
      - entities that are in a room that appear in any ClientLeaveRoom/ClientEnterRoom



- Replication Systems:
  - EntitySpawn:
    - we can have ReplicationMode::room or ReplicationMode::force. Force means we always replicate to everyone, without caring baout rooms
    - check through all ClientGainedVisibility -> send SpawnEntity
    - check through all clients in cache -> send SpawnEntity
  - ComponengUpdate:
    - check through all ClientGainedVisibility -> send InsertComponent
    - check through all clients in cache -> send ComponentUpdate if component changed, ComponentInsert if added

- Or should we separate the modes:
  - Only(id)/Except(id) from the rooms?

- for replication, we check all entities that have Replicate.
  - we check the list of rooms they belong in.
  - for each room, if the client belongs to that room, we replicate to that client

- when an entity or player leaves a room, check all the entities that won't get replicated to that player anymore
  - the entity doesn't get replicated anymore.
    - OPTION 1: for each of them, add a client Component LostVisibility. This component means that the entity is not visible to that client anymore, but still exists
      - if the client rejoins the room soon after (~1s), we remove the LostVisibility component
      - the main benefit is that an entity leaves/rejoins a room frequently, we don't have to keep spawning/despawning it.
    - OPTION 2: we despawn the entity on the client 
      - but careful if that despawn arrives after a spawn (that was sent after the despawn) -> maybe not possible if we use sequenced ordering for entity actions?


- Examples:
  - a new client connects. We go through every entity. All entity who are in `Only(id)` or `All` special rooms get replicated to that client
    - via ComponentInsert ideally
  - a client joins a room. We go through every entity. All entity who are in `Only(id)` or `All` or `RoomId` rooms get replicated to that client
    - we iterate through all entities with that component
    - If the entity has that client in its cache, that means we were already replicating to that client, check if component changed and replicate if so
    - If the entity doesn't have that client in its cache, that means we were not replicating to that client, check if component changed and replicate if so
    - via ComponentInsert ideally