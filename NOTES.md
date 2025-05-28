https://excalidraw.com/#room=6556b82e2cc953b853cd,eIOMjgsfWiA7iaFzjk1blA

# Examples

TODO:
- add button on server to add a new local client, or add a new client in a separate app
- the confirmed.tick is not the same than the rollback_start_tick sometimes.. It looks like the confirmed.tick
  for some entities is still at an OLD rollback tick, meaning that the confirmed tick did not get updated.
- we have temporary ChannelNotFound error in avian_3d example when a client connects?

- interpolation:
  - sync all components at the same time from confirmed to interpolated. We sync them all at once, but we make the entity
    visible only after a certain period of time has passed! (we have 2 updates to interpolate between)

STATUS:
- SimpleBox:
  - runs ok in client/server.
  - host-server not tested
- Priority:
  - runs ok in client/server
  - host-server not tested
- ReplicationGorups:
  - runs ok in client/server
  - host-server not tested
- ClientReplication
  - runs ok in client/server
  - host-server not tested
  - the delete input doesn't get received by the server because the entity is deleted immediately. Is that just because of no link conditioner?
- avian_physics:
  - runs ok in client/server
  - Why are there duplicate walls spawned on server?
  - We should provide pre-computed functions for avian to check rollbacks with a certain tolerance.
  - Collisions still provide constant rollbacks! Might just get fixed by adding some tolerance?
  - Collisions with wall causes rollbacks, but otherwise its fine!
- fps:
  - runs ok in client/server
  - sometimes interpolated is frozen and predicted bot causes constant rollbacks, sync issue?
- avian 3d:
  - runs ok in client/server
  - still constant rollbacks because of contacts
  - ITS POSSIBLE TO GET COMPLETELY DESYNCED FOR SOME REASON, AND ITS NOT ROLLBACKED!
  - STILL HAVE DOUBLE BULLET SPAWN even though we use JUST_PRESSED! It looks like the leafwing ActionState is always JustPressed
  - possible for the client to get 'stuck'
- spaceships
  - runs ok in client/server
  - Weapon causes rollback, the Confirmed component does not have last_fire_tick for some reason! Is it not being replicated correctly?


- Have a pane where you can see the running clients, servers, processes.


TODO:
- SUPER dangerous that if you disable some plugins, some components might not be registered at the same time on 2 peers. Need a protocol check at the beginning to make sure that the protocols are the same!
- update examples
- update docstrings
- update book
- run benchmarks, and update how we write replication packets?
- add unit test for replicating entities between the ServerSendInterval (i.e. with ServerSendInterval which is not every tick)
- on the server, we get cases where the input buffer just contains [SameAsPrecedent]. Normally
  the first value should never be just SameAsPrecedent! That's due to `update_buffer` using `set_raw`. But maybe that's ok? if there's only SameAsPrecedent, we don't do anything (i.e. we re-use the existing inputs)
- ON DISCONNECTION, we reset ReplicateionReceiver, ReplicationSender, Transport to their default state.
  - But maybe we should to the reset on Connection, so that if users changed something it gets taken into account?
  - Also we might need to reset the MessageReceiver/MessageSender!

- in netcode: the difference between `send_packets` and `send_netcode_packets` is a bit awkward...
- add compression in Transport before splitting bytes into fragments.
- refactor replication to not use hashmaps if no replication group is used.
    

NEEDS UNIT TEST:
- check that we don't send replication-updates for entities that are not visible
- check that 'send_tick' and replication change_ticks work correctly
- check the PredictionTarget for ReplicateLike is not using the root's PredictionTarget
- check that the relationship between Link and Connect works correctly.
  - Adding Unlinked should trigger Disconnect (or add Disconnected)
  - Trigger Connect
    - we add Connecting
    - we trigger LinkStart
    -> ALTERNATIVE: the udp can keep working even if disconnected?

BUGS:
- It looks like when we batch insert component values in replication, we insert some duplicate component ids!
  - the temp_write_buffer doesn't seem to get fully erased between frames?
- things break down with no conditioner because the client seems to sometimes be slightly ahead of server?
  - actually it's deeper than that! What is going on ??
  - the RemoteTimeline offset keeps increasing infinitely!
- when we receive a SyncEvent, we seem to go into a negative spiral. Maybe updating the InputBuffer to use the new ticks is incorrect?

# Server sending messages

- We need a ServerMessageSender on the server, where you can serialize a message once and it will be sent to a subset
  of network targets (ClientOf) of that server.


# Status:

- WebSocket/Steam: TODO
- Replication:
  - ReplicationGroup timer + priority
  - HostServer handling
  - Authority
  - need to add tests for sender/receive/authority/hierarchy/delta
- Receive:
  - re-add UpdateConfirmedTick in apply_world

- EntityMap: maybe this should be directly on the Serialize level?

TODOs:
- Maybe have a DummyConnection that is just pass-through on top of the io? and then everything else can react to Connected/Disconnected events?
  Maybe we can have some Access or bitmask of all connections, and the access could have `accept_all`, etc.?
 
- Most systems (for example ReplicationPlugin), need to only run if the timeline is synced, or if the senders are connected.
  How to handle this gracefully?
  

TEST TODO:
 - There seems to be a very weird ordering issue? Adding MessageDirection messed up some of the message receive/send
 - ReplicationSender needs to send a message at the start with their replication interval so that the receiver can adjust the interpolation timeline.

# Messages

- Maybe we remove NetworkDirection, and the user can add required_components to specify which of their entities 
  will add which components?
  - and we add by default some required components, for example for Inputs?

- Need to find a way to broadcast messages from other players to a player.
  - option 1: all messages are wrapped with FromPeer<M> which indicates the original sender?
   
- Client sends message to Server, with a given target.
  - maybe it's wrapped as BroadcastMessage<>?
  - the client-of receives it
  - the target is mapped to the correct client-of entities on the server
    
- or the message itself should support it? i.e. we have InputMessage<> (for the server) and RebroadcastInput? Rebroadcast input gets added only if the user requests it in the registry.


# Server

- how can the server send messages to multiple peers?
- OPTION 1:
  - `Server` component has `send_message_to_target(NetworkTarget)` will find the subset of clients that should have the message sent, and buffer on each message sender?
- OPTION 2:
  - global resource that maps from network targets (from Link or Netcode) to entity.
  - command/trigger `send_message_to_target` -> find all the entities that have a MessageSender that match
    and buffer the message


# Host-server

- You have a server with n ClientOf.
- HostServer 
  - The Server will have its own ServerMessageSender? send_to_target() and kllllll
  - OPTION1: one of the ClientOfs is also a Client? with Transport via Channels? has a an extra component Host/Local
    - PredictionManager and InterpolationManager are disabled
    - Messages hjj
  - OPTION2: the ClientOf has a Link but no io (or just a dummy io). 



# Replication

- Maybe the core replication plugin is only for a single connection
  - server adds extra stuff on top of that to serialize once between multiple connections?
  - maybe we don't need to serialize once for every connection, and serializing once per connection is enough?
- Again, you register components independently from client or server.
But if you add a direction we will handle it automatically for the client-server case

- a ReplicationPlugin:
  - you add a ReplicateOn Relationship and we replicate on every entity that has a link + transport + message-manager
  - server-only: ReplicateTo(server_entity, network_target) component can be added on the server, which adds ReplicateOn
      on each ClientOf that matches the network_target
  - in this paradigm the replication is done independently on each connection, so we serialize separately for each connection.
    This is a big difference from lightyear where the server serializes only once if possible.

- Visibility:
  - by default an entity is replicated to all clients specified in Senders.
  - you can add NetworkVisibility on the replicated entity; where you specify which clients lost/gain visibility
  - you can also add SenderNetworkVisibility on a sender, to specify which clients lost/gain visibility
   
  - Maybe enable whitelist/blacklist (currently it's only whitelist, i.e. by default no entities are visible). Can be done by simply adding all clients once as visible?

- Hierarchy:
  - ReplicateLike is added recursively to all children.
    - Hooks:
      - when ReplicateLike is added, should we update the Sender's replicated entities? or should we add Replicate?
        Or do we also go through every entity that is in ReplicatedEntities?

  - Other idea: 
    - the 'root entity' is the ReplicationGroup. All children or ReplicateLike are part of the group.
      - then the SendInterval data is part of the ReplicationGroup.


- How to start replication?
  - ideally we would have M:N relationships. because one sender replicated many entities, and each entity can be replicated by multiple senders.
  - **right now we will constrain each entity to be only replicated by one sender.**
    - ReplicateOn<Entity> -> sender will replicate that entity 
    - ReplicateOnServer<Entity> -> each sender that is a ClientOf that entity will replicate it.
    - Ideally we also want the relationship to hold data. For example `is_replicating`, etc.
  - Should we do it by triggers? i.e. you trigger ReplicateOn<Entity> for your target entity?

- ReplicationGroup:
  - the ReplicationSender has an internal timer, and the ReplicationGroup has one too?
  - We could have one entity per replication group? Maybe not, as the replication group is specific to this one sender
  - is a relationship, can be added by the user to specify that multiple entities must be replicated together
  - The group is separate entity that can contain specific metadata for the group
  - if there is no ReplicationGroup, we can assume that the entity is part of the 'default' group entity.
  - The ReplicationGroup defines:
    - priority
    - how often messages are sent (send_timer)


# IDEAS

- Authority: we want seamless authority transfers so states where the client is connected to 2 servers; maybe it starts accepting the
  packets from both server for a while, buffers them internally (instead of applying to world), and the AuthorityTransfer has a Tick after
  which the transfer is truly effective. We can have Transferring/Transferred.


- we want integration tests
  - with client-server with netcode
  - P2P where we just use Link; maybe PeerId::Link


# IO
RECEIVE FLOW
- Server receives (packet, socketAddr)
- Spawns a ClientOf with PeerId::IP(SocketAddr), with State = Connecting
- passes (packet, socketAddr) to Netcode (via the normal netcode process?)
- netcode returns that it's a connection -> Update the PeerId::Netcode(u64), State = Connected.
  - The server-map has both PeerId::Netcode and PeerId::IP as keys :)

SEND FLOW
- Link has a packet to send
- With ClientOf, we find the ServerUdpIO on the Server
- send

- each Link has a remote PeerId and a local PeerId. The default is PeerId::Entity, where we just rely on the Entities for identification
- Peer is the term we use for client-agnostic IO
- There is client-IO (connects only to one peer) or Server-IO (listens for clients to connect)
  - server: once we get a client, we spawn a new ClientOf and we assign a PeerId. The PeerId can also just be the PeerId::Entity (in which case we use the entity), for example for Channels
  - client/peer: once we get connected, we get a PeerId for the remote and for the Local
- Some IOs directly give you a PeerId: Steam

- Other IOs give you an initial PeerId (for example WebTransport gives you a SocketAddr). You can keep using that, or if you're in client-server mode you can apply the Netcode layer
  to replace that PeerId on the link with a Netcode-related id: PeerId::Netcode(u64)

- Actually we want to completely separate PeerId from LinkId
  - PeerId is purely a client-server information.
  - LinkId is purely a link-based information
  - We have PeerId::FromLink(LinkId)
  -> it's because we receive from ServerUdp, so we assign PeerId::Udp(Socket)
     Then we go through netcode, so we override PeerId::Netcode()
     But then if netcode gets disconnected, we lose the PeerId but we should still have the LinkId!

- NETCODE:
  - the problem we have is that before, we had different buffers for send-payload and netcode payload.
    i.e. in receive, we would prepare ConnectionResponse packets immediately and send them via the io
  - now we buffer them in link.send, but we are not allowed to send them because the client is not connected yet!


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

- The tick should be specific to the timeline/connection!
  - on server/client, we have a LocalTimeline. The tick gets incremented every FixedUpdate, and the overstep
  - on server, we can define a LocalTimeline, where the Tick is incremented every FixedUpdate.
  - the transport should be using the LocalTimeline to get the tick information.


# Refactoring

- Io: raw io (channels, WebTransport, Websocket)
- Link/Transport: component that will store raw bytes that we receive/send from the network (webtransport, udp, etc.). Link between two peers to send data.
- Channel: component adds reliability by allowing you to specify multiple channels depending on the the channel_id, we buffer the stuff to be processed on the channels
- Session: component that tracks the long-term state of the link.




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

