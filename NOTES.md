# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
  - use local executors for async, and use one process/thread per core instead of doing multi-threading (more complicated and less performant
  - one server: 1 game room per core?

PROBLEMS/BUGS:
- more rollbacks than expected (Especially with rooms). Let's check what happens with 0 jitter -> there should be 0 rollback no?
  - is it because we are sending too much data to the client? (a lot of entity spawn/despawn)
  - is it because the ticks are not in sync?
  - with 0 jitter, the problem completely disappears, prediction becomes butter smooth
  - with higher send_interval, the prediction becomes extremely jittery!!!
  - It could be something like:
    - we are sending the player position update for a tick T1 in a packet
      is only received after. So at tick 20, we think we have the world state for tick 20, but we don't. We don't even have the world state for tick 18.

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