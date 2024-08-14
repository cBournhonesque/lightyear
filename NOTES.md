# Entity mapping

- SOLUTION 0: the entity receiver always keeps the mapping.
  - we always try to apply the mapping from remote->local on receive side
  - we always try to apply the mapping from local->remote on send side.
  - C1 spawns E1, sends to S, S spawns E0. S replicated to C2 who spawns E2
    C1 sends a message about E1, entity gets mapped to E0.
    S rebroadcasts to C1, mapping from E0 to E1.
    S rebroadcasts to C2, no mapping. The receiver maps E0 to E2.
  - We know there are no conflicts because only the receiver has the mapping!
    - i.e. we cannot be in a situation where C maps to the remote and sends it to S, and S receives it,
      tries to map it from remote to local, but fails because it was already mapped!
      

- SOLUTION 1: the client always keeps the mapping
  - if S spawns and sends the client, client has the mapping. When client sends a message about the entity, it 
    converts it using the mapping. If client receives a message about the entity, it convers it using the mapping.
  - if C spawns the entity, it sends it to S. S spawns an entity, sends a message back to C containing the mapping.
    From this point on C has the mapping.
  - CONS:
    - additional bandwidth
    - there's a period of time where no mapping is available
    - is it compatible with simulator vs replication-server

- SOLUTION 2: the receiver always keeps the mapping
  - C1 spawns E1, sends it to S which spawns E2. 
    - S adds E2<>E1 to its mapping
    - when S sends a component or message to C1; if the entity contains Replicated that means we are the receiver,
      so we apply entity mapping? i.e. the message contains E2, but is converted to E1 at sending time.
      Or if the entity is in the local-map we apply the mapping? i.e. we always try to refer to entities in the local World,
    - when C1 sends a message related to E1, we know we are the sender (because Replicated is missing, so we don't apply entity mapping. The receiver will apply mapping)
  - S spawns E0, sends it to C1 which spawns E1, sends it to C2 which spawns E2
    - C1 adds E1<>E0 in its mapping, etc.
    - when S sends a message, it knows it is not Replicated so its not the sender so it doesn't apply any mapping.
    - when C sends a message about E1, it knows it is the receiver, etc.
  - On Authority Transfer we need to remove `Replicated` also, and maybe update the entity mappings.
  - CONS:
    - the mappers are more complicated
    - what happens on authority transfer where the receiver becomes the sender?

- SOLUTION 3: replication-server
  - when we spawn an entity to replicate. We ask the replication-server to spawn it, it spawns it (whether we are client 
    or server), we receive it and can update our mapping. Each client/simulator is the receiver and can always do mapping
  - we could also do this approach right now. Spawning an entity means that the 
  - CONS:
    - need a way to handle pre-spawned entities.
    - need to implement replication-server
  


# Replication Server

- Introduce a ReplicationServer and a Simulator?
  - the ReplicationServer handles:
    - forwarding messages between the clients (same as the current server)
    - holds the state of the world
    - maintains an entity mapping 
  - the simulator is basically the same as a normal client. Just no input handling, prediction, interpolation.
  - basically ReplicationServer = server, and we can just add the concept of 'simulators' which are like clients. The difference is that the ReplicationServer should not run any simulation systems,
    it just replicates to all clients/simulators.
  - clients AND simulator would have the current server replication components:
    - ReplicationTarget that specifies other clients and simulators
    - SyncTarget to specify who should predict/interpolate? ideally the clients should decide for themselves
    - NetworkRelevanceMode: visibility of who receives the entity
    - ControlledBy: connects/disconnects are sent to every client?
    - ReplicationGroup: ok
    - Hierarchy: ok
  - transferring authority would then mean: sending all the replication components to the new owner, and removing them from the old owner.

  - Actually we could try this:
    - all components of Replication are shared between clients and server, they are also replicable.
    - So when you transfer authority, you just replicate the Authority component to the new owner.
    - On the client if you add `ReplicationTarget`, `SyncTarget`, nothing happens, but those get replicated to the Server. The server then uses them for the replication behaviour. One big problem is ReplicationGroup? I guess we can just apply EntityMapping.
    - How do we avoid bidirectional replication? The component `Authority` specifies who is sending updates. You only send updates
      if you have authority. That means that the server and the client both have the `Replicate` bundle but only one of them is sending updates?
    - Case 1: server spawns an entity and has authority
      - the Replicate bundle is added, and is used to define how its replicated. 
      - no need to replicate the Replicate bundle
      - the Authority component is added, so 
      - Case 3: it transfers authority to a client.
        - 
    - Case 2: client spawns an entity and has authority, it wants to replicate to all other players
      - the Replicate bundle is added, and is used to define how its replicated. Most of the components are not actually used.
      - the Replicate bundle is replicated to the server, which then uses it to replicate to all other clients.
      - Careful to use ReplicationTarget::AllExceptSingle!!! otherwise all hell breaks loose
      - the server has an Authority component which tracks which peer has authority (Self or Client). 
      - In particular, it WILL NOT ACCEPT REPLICATION UPDATES FROM A PEER THAT DOES NOT HAVE AUTHORITY!
      - the clients also have this Authority component?

  - Having Authority means:
    - I don't accept replication updates from another peer
    - server tracks at all times who has authority over an entity
    - server does not accept receiving updates from a peer with no authority
  - TransferAuthority:
    - used from the server to client to transfer authority on the client
    - from server to client: (server gives authority to client)
        - client gains `HasAuthority`
        - server updates `AuthorityPeer`
    - from client 1 to client 2: (server takes authority from C1 and gives it to C2)
      - send a message to both
      - they update accordingly
    - from client to server: (server takes authority from client)
  - GetAuthorityRequest:
    - used from client to get authority over an entity. Client to Server msg
      - server can refused if it already got a previous request
      - from client to server: client adds the `HasAuthority` component. It does not accept receiving updates from the server anymore.

TODO:
- [x]: we receive updates only if (server) the sender has authority, or if (client) we don't have authority.
- [x]: (on client) we only send updates if we have authority
- [x]: add TransferAuthority command on the client
- [ ]: update hierarchy
- [ ]: receive edge cases:
  - [x]: server adds AuthorityPeer when a client replicates to it
  - [ ]: also refuse entity-actions if the sender does not have authority?
- [ ]: send edge cases:
  - [ ]: what happens on component removal?
- [ ]: handle AuthorityChange messages on the clients
- [ ]: think about what happens in PrePredicted
    
       

- What would P2P look like?
  - the ReplicationServer is also the client! Either in different Worlds (to keep visibility, etc.),
    or in the same World
  - the Simulator can also be merged with the client World? i.e. a client can be both a Simulator and a Client.

- ReplicationServer (server) contains the full entity mapping
  - client 1 spawns E1, sends it to the RS which spawns E1*. RS replicates it to other clients/simulators.

# Authority Transfer

```rust
enum AuthorityOwner {
    Server,
    Client(ClientId),
    // the entity becomes orphaned and are not simulated
    None,
}
```
- Having authority means that we have the burden of simulating the entity
  - for example, server has authority and the other clients just receive replication updates
  - if a client has authority, it simulates it and sends replication updates to the server. The server also sends replication updates.
  - the component `Authority` symbolises that we have authority over the entity. There can be only 1 peer with authority

- ConnectionManager.request_authority(entity);
  - if the client requests, the server will route the request to the current owner of the entity (possibly itself)
  - if the server is requesting, the server will send the request to the current owner of the entity
- V1: all requests are successful. 
  - on receiving the request. We return a message that indicate that the transfer is successful.
  - The entity_maps don't seem like they need to be updated
  - the previous owner will remove Replicating, and add Replicated? it should also remove ReplicationTarget or ReplicateToServer, but without
    despawning the entity.
    - if the entity is transferred from C1 to C2, the server needs to its replication target. Maybe the server can just listen for `SuccessfulTransfer` and then update its replication target accordingly?
    - if the entity is transferred from C1 to S, the server needs to update its replication target. (i.e. if it notices that it is the new Authority owner)
      C1 loses ReplicationToServer and Replicating, and adds Replicated.
      
    
  - the new owner receives TransferSuccessful message and adds ReplicationTarget component.
  
- commands.transfer_authority(entity, AuthorityOwner):
  - send transfer 
  - remove Replicate on the entity
  - send a message to the new owner to add the Replicate component
  - 


# Serialization

- Replication serialization:
  - to improve performance, we want to:
    - entities that don't have SerializationGroup are considered part of SerializationGroup for the PLACEHOLDER entity (this should be ok since the PLACEHOLDER is never instantiated)
    - we need to manually split the Updates messages for the PLACEHOLDER group into multiple packets if they are too big
    - we include Option<ReplicationGroup> and use only 1 byte for the placeholder, or we could set it as a Channel, so that we use one 1 byte total! (channel ids use 1 byte up to 64 channels)
    - we can only set priority on replication groups, still
    - if an entity has a priority, then they need to use a replication group? i.e we recreate a replication group for them?
    - to write the ActualMessage:
      - serialize the message_id


- Serialization strategy:
  - SEND:
    - we need to send individual messages early (because we don't want to clone the data, and we want to serialize only once even when sending to multiple clients), so we allocate a buffer for each message and serialize the data inside.
       - maybe use an arena allocator at this stage so that all new messages are allocated quickly
         (we only need them up to the end of the frame)
       - this buffer then gets stored in the channels
       - maybe it would gain to be a bytes for reliable channels (even after sending once), we need to store the message until we receive an ack
       - Or can we reuse an existing buffer where we put all the messages that we clear after sending?
    - for replication, we sometimes serialize components individually and buffer them before we can write the final message.
      - we know the component is still owned by the world and not removed/changed at this point,
        so we could just store the raw pointer + ComponentNetId at this point and serialize later in one go using the component registry? But the Ptr wouldn't work anymore because we're not querying, no?
      - we could try to build the final message directly to avoid allocating once the structure to store the individual component data, and once to build the final message
    - when building the packet to send, we can allocate a big buffer (of size MTU), then
      iterate through the channels to find the packets that are ready, and pack them into the 
      final packet. That buffer can just be a Vec<u8> since we are not doing any splitting.
  - RECEIVE:
    - we receive the bytes from the io, and we can store them in a big Bytes.
    - we want to be able to read parts of it (header, channel_id) but then parts of the big Bytes 
      would be stored in channel receivers. Hopefully using Bytes we can avoid allocating?
      We can use `Bytes::slice` to create a new Bytes from a subset of the original Bytes!
      -> DONE!
    - When reading the replication messages, we can do the same trick where we split the bytes for each component? but then it's a waste because we need to store the length of the bytes. More efficient if we just read directly inline. 


- Replication current policy:
  - send all updates since last ACK tick for that entity.
- Replication new policy: send all updates since last send
  - for an entity E, keep track of ACK tick, send tick, and change tick.
  - if the entity changed (i.e. change_tick > send_tick), send update and set send_tick to change_tick.
  - Save for entity E that we sent a message at tick T
  - If message is receives, bump ACK tick to send_tick
  - Message is considered lost if we didn't receive an ack after 1.5 * RTT. (i.e. we didn't receive an ack after send_tick + 1.5 * RTT)
    if that's the case, send the send_tick back to ACK_TICK, so that we need to send the message again.
 

- needs tests for: 
  - update at tick 10, send_tick = 10, ack_tick = None,
  - update at tick 12, send_tick = 12, ack_tick = None,
  - ack tick 10, send_tick = 12, ack_tick = 10,
  - lost tick 12, send_tick = 10, ack_tick = 10 (we revert send_tick to ack_tick)

  - update at tick 10, send_tick = 10, ack_tick = None,
  - update at tick 12, send_tick = 12, ack_tick = None,
  - lost tick 12, send_tick = 10, ack_tick = None (we revert send_tick to ack_tick)
  - ack tick 10, send_tick = 10, ack_tick = 10,

  - update at tick 10, send_tick = 10, ack_tick = None
  - update at tick

  - saves bandwidth because 99% of packets should arrive correctly.


# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
    - use local executors for async, and use one process/thread per core instead of doing multi-threading (more
      complicated and less performant
    - one server: 1 game room per core?

- TODO: create an example related to cheating, where the server can validate inputs


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