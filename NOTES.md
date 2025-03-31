# Replication

- Maybe the core replication plugin is only for a single connection 
  - server adds extra stuff on top of that to serialize once between multiple connections? 
  - maybe we don't need to serialize once for every connection, and serializing once per connection is enough?
- Again, you register components independently from client or server.
But if you add a direction we will handle it automatically for the client-server case

- ReplicationGroup: 
  - is a relationship, can be added by the user to specify that multiple entities must be replicated together
  - The group is separate entity that can contain specific metadata for the group
  - if there is no ReplicationGroup, we can assume that the entity is part of the 'default' group entity.


# SyncPlugin

- Shared: adds Ping/Pong messages
- Client:
  - adds 2 custom time: Time<Predicted> and Time<Interpolated> + Maybe Time<Server>?
- Server: handle Ping/Pong messages

- Create a separate crate with Client/Server/Shared
- create features [client] and [server] for the client/server part
- provide a unique SyncPlugin in shared, that also calls the client/server part depending on the feature
- the lightyear_client and lightyear_server will call the appropirate feature

- STRETCH: maybe we could just do Sync between a Master and a Follower?

- PingManager is a component present alongside Transport, to send and receive pings
  and define the Timers at which we send/receive ping. Both client/server have it
  - it uses Time<Real> in its systems to decide how often to receive/send pings, i.e to tick the PingTimer
  - However the pings themselves contain information about the WrappedTime estimate on both client and server, so that they can sync
  - maybe it's not actually needed, as we can use the packet acks to estimate the RTT/jitter? however those messages don't contain the WrappedTime
  - the timeline is 
    - PREUPDATE: time ticks
    - POSTUPDATE: Client prepare Ping, record time A in store
    - Client send Ping to link, time B
    - Client io send Ping time C
    - Server io receive Ping, time D
    - Server MessageReceiver receive Ping, time E
    - PREUPDATE: Server processes Ping from MessageReceiver, time F. Prepares Pong with ping_receive_time = F, pong_send_time = NOT_SET
    - POSTUPDATE: Server send Pong to MessageSender time G, sets pong_send_time = G
    - Server io send Pong time H
    - Client io receive Pong time I
    - Client MessageReceiver receive Pong time J. Process Pong:
      - real RTT = (D-C)+(I-H)
      - recorded RTT = (J-A)-(server_process_time=G-F)
        - we should also remove the client_process_time=B-A, or send pings in PostUpdate?
  - ping/pong let's us compute the RTT/Jitter. The pong's received/send is just used to estimate the time spent inside the server
- We have a Timeline trait implemented by multiple timelines:
      - Predicted
      - Interpolated
      - Server
  - The timeline trait lets you speed up/slow down the timeline, as well as know the 
    WrappedTime, which is the number of ms since the start of the server/master


- It looks like we never rewind the timeline in the past, so we coudl make them a Time<Predicted>, etc.
- For sync:
  - the client clock always progresses forward, but sometimes goes slower/faster
  - the Client Tick is set based on the FixedUpdate schedule, i.e. making the time go faster/slower will make the tick stay in sync with the server
  - If there's too much of a desync, we can just reset the client tick to the server tick
  - the client ideal time should be the server time + the RTT/2
    - we can represent server time as a server_tick + overstep
    - the Pong should contain overstep + pong-ping time


# Lightyear client

- You specify a protocol where you register messages/channels
  - channels can have a direction, so do the messages
- Creating a Client:
  - you insert your ClientConnection marker with the correct type (Netcode, etc.) which also inserts the Client marker
  - you insert a Client marker component on an entity, which adds:
  - a Transport, with all channel senders marked ClientToServer, all channel receivers marked ServerToClient
  - a Link, 
  - MessageSenders / MessageReceivers, etc.
  - you will need to insert the io yourself

- By default you register Channels/Messages with no direction (i.e. they won't be auto-added to Client/Server)
    - You can specify a direction for a Message, and we will add the required components MessageSender<M>/Receiver<M> on Client/Server
    - You can specify a direction for a Channel, and we will make sure that Transport.add_sender_from_registry<C> will be called on Client/Server depending on direction
    - Right now we rebuild ConnectionManager on Connect attempt, so we we could do the same thing if Connect is triggered on the Client entity.
    - (and OnDisconnect we remove all the MessageReceiver, etc. from the entity)

# Messages

- Should MessageRegistry be stored on the Transport?

# Refactoring

- Io: raw io (channels, WebTransport, Websocket)
- Link/Transport: component that will store raw bytes that we receive/send from the network (webtransport, udp, etc.). Link between two peers to send data.
- Channel: component adds reliability by allowing you to specify multiple channels depending on the the channel_id, we buffer the stuff to be processed on the channels
- Session: component that tracks the long-term state of the link.


WITHOUT CONNECTION
When we receive:
- we poll from the io and store the result in the Session which stores the packets
- we read from the Session to get packets, and process them in the Transport buffer (which adds reliability, fragmentation, etc.)
- from the Transport we receive Messages (ChannelKind + Tick + Bytes)

When we send:
- We buffer a message in the Channel
- the Transport will flush all ready packets into the Link
- the raw Io takes from the Link and sends it through the network

WITH CONNECTION
When we receive on client:
- we poll the IOs to add all received network packets in the Link
- we poll the connection, which gives us packets.
    - the connection could be using the Link to get packets (for example Netcode, i.e. which takes
      the buffer from the link)
      or it could just provide the packets from some internal mechanism (for example Steam)
- we put the packets in the Transport

When we receive on server:
- we poll any IOs that are linked via ConnectedOn to a ServerConnection, those packets are stored on the Link
- the connection will receive():
  - either internal mechanism
  - either it looks at all entities that are ConnectedOn and takes from their Link

When we send:
- we buffer a message in the Channel (an entity can have multiple channels so you can write to them in parallel). They are distinguished by `Channel<C>`. (Transport)
- we Channel will flush all ready packets into the Link
- for each of these packets, we call Connection::send
  - steam will just buffer them with an internal mechanism
  - Netcode will do extra processing

- we buffer message into the Transport
- regular system that polls the Transport, identifies messages that are ready to be sent, and puts them in a local buffer on the Transport
- Connection system:
  - will take these messages from the Transport, and put them in the Link
- Io system will take messages from the link and send them through IO

If you just want to send unreliable messages without a connection (i.e. just use Transport + Link without connection), you could also create a DummyConnection
that just takes the message from the Transport and puts it in the Link.

Systems:
- the problem is that NetcodeConnection and SteamConnection would look very similar.
Their systems would: poll the Transport on the entity for any packets ready to send


COMPONENTS
- we store Channel<C> as that's what users buffer messages into.
  - do we store the Channel<C> components on the same entity as the Link? or do we make a relationship between them? A relationship means that we could iterate in parallel
  - we store a separate ChannelSender<C> on each entity so that users can send messages in parallel, and a Channel<C>
  - or do we create a Transport component that contains multiple Channels? (the problem is that if you want to write to one channel it blocks the others)
- do we store both the Link and the Session?

ENTITIES
- Client:
  - one entity per link that stores the Io, Link, Transport (Channel<C>), ClientConnection
  - this means we could be connected to multiple remote peers at the same time
- Server:
  - we want to be able to support multiple connections at the same time, so one entity per ServerConnection. Each ServerConnection can have a relationship with multiple entities, which are each of
    the entities connected on that connection. Each of the entities have a `ConnectedOn(ServerConnection entity)` component
  - one entity per client, with the ConnectedOn component, an Io component, a Link component, a Channel component


# BEVY

I noticed that when system ordering is ambiguous, once my app is compiled, the system order is completely fixed (i.e. all the ambiguous systems get ordered in a given fixed order). Is there a way to get what that fixed system order is?
Is it the cached topsort of the schedule?
Does bevy_mod_debugdump show that 'fixed' system order?
It would be immensely useful to know what the exact system order being used is, so that I can debug which ambiguity is causing a given bug.
Should I create an issue for something like this? I would love the editor to somehow provide tools to help me debug ambiguities 


# PrePrediction

- Current situation:
  - client spawns E1 with PrePredicted containing E1
  - E1 gets replicated to Server, who spawns E2 with Replicate
  - E2 gets replicated to the client, with PrePredicted containing E1
  - client spawns E3 with Confirmed, figures out that E1 is the Predicted associated with E3

- The problem:
  - server receives E1 and maintains a E2<>E1 mapping.
  - when server replicates back E2, it will map it back to E1

The solution:
  - client spawns E1 with confirmed, E2 with predicted, and add E1<>E2 predicted mapping
  - it replicates E1 to server which spawns E3 and has E1<>E3.
  - server transfers authority for E1 to itself.

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
  - [ ]: server adds HasAuthority if entity is spawned with AuthorityPeer::Server
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
