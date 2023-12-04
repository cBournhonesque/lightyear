# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
  - use local executors for async, and use one process/thread per core instead of doing multi-threading (more complicated and less performant
  - one server: 1 game room per core?

PROBLEMS/BUGS:
- None right now

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