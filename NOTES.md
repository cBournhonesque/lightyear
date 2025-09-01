Issue with correction/visual interpolation

Frame 1
- we had local at -30.0, stable
- after rollback, we get -10.0 for some reason (1)
- we get a correction of -20.0
FI: stable at -30.0
Correction: we get something like -28.

Frame 2
- restore the tick value at -10.0
FI: Stable at -10.0 -> WEIRD? (2)
Correction: we get -27


0) Position and Transform are not in sync in PostUpdate!!

1) How come there is a misprediction for the local entity? Check input logs

2) The FrameInterpolation should be using the previous Visual value.
Actually maybe not because the FI is just to interpolate between Fixed updates.


-------------------------


Great question. Photon Fusion’s “projectile data buffer” (a small, networked ring/circular buffer) is a state‑based way to replicate shots. It’s favored over “one network event/RPC per shot” because it rides on Fusion’s normal snapshot/state replication instead of creating a separate stream of messages.

1) Why a ring buffer beats “send an event every time the gun fires”

It’s state‑based, not message‑based.
With a ring buffer the authoritative object (usually the weapon owner) exposes two pieces of state:

a monotonic counter (e.g., fireCount = “number of shots ever fired”), and

a fixed‑size array of ProjectileData slots used as a circular log.

On each shot the authority overwrites buffer[fireCount % capacity] with the new ProjectileData and increments fireCount. Clients render anything whose sequence they haven’t shown yet. Fusion’s official sample calls this out explicitly: a fixed array acting as a circular buffer, with clients catching up using their local head (“visibleFireCount”).
Photon Engine

Much less fragile under loss/out‑of‑order.
If a packet is dropped, the next snapshot still carries the latest fireCount and the changed slots. Clients simply “catch up” to that count. You don’t need per‑event reliability/ack logic or to replay missed shots; the next state snapshot fixes everything. This maps to Fusion’s snapshot replication model where clients constantly replace predicted state with the authoritative state for the snapshot’s tick.
Photon Engine

Delta/compression friendly.
Fusion only transmits changes to networked properties/collections. Updating a couple of array entries and the counter will send just those deltas, not the whole buffer every tick. (Photon: “Fusion always sends only the delta between peers.”) The engine’s state‑transfer path includes delta compression by design.
forum.photonengine.com
Photon Engine

Aggregates bursts cheaply.
If a weapon fires several times between two snapshots, the buffer accumulates those shots and they arrive together via normal state replication. With per‑shot RPC/events you pay header + reliability costs per event.

Interest management is simpler.
Because the buffer lives on the controlling object (player/weapon), standard interest culling applies—clients only receive that object’s state if they should see it. The samples highlight this advantage.
Photon Engine

Good default for most projectile types.
Photon’s Projectiles Essentials and Projectiles Advanced samples recommend and center around this buffer approach for hitscan and many kinematic projectiles (visuals are local Unity objects; you replicate only the sparse data needed to reconstruct the path).
Photon Engine
+2
Photon Engine
+2

Caveat: A ring buffer tied to the owner object can’t outlive that object. For “very long‑living/important” projectiles you might use a dedicated networked object or different manager pattern. The docs call this out.
Photon Engine

2) Minimal Rust sketch (data structure + client usage)

Below is a compact, engine‑agnostic sketch of the idea. Think of head_seq like Fusion’s fireCount, and next_seq like a client’s visibleFireCount. The buffer index is always seq % CAPACITY. (In Fusion C#, the sample does exactly this.
Photon Engine
)

#[derive(Copy, Clone, Default, Debug)]
struct Vec3 { x: f32, y: f32, z: f32 }

#[derive(Copy, Clone, Default, Debug)]
struct ProjectileData {
    fire_tick: u32,            // or use a separate monotonic shot counter
    origin: Vec3,              // muzzle position at fire time
    dir: Vec3,                 // normalized direction (or velocity)
    hit_pos: Option<Vec3>,     // hitscan: where it hit (if known)
    finish_tick: Option<u32>,  // when it hit/expired (kinematic)
}

struct RingBuffer<T: Copy + Default, const N: usize> {
    buf: [T; N],
    head_seq: u64, // total items ever written (monotonic)
}

impl<T: Copy + Default, const N: usize> RingBuffer<T, N> {
    fn new() -> Self { Self { buf: [T::default(); N], head_seq: 0 } }
    fn capacity(&self) -> usize { N }
    fn idx(&self, seq: u64) -> usize { (seq % N as u64) as usize }

    /// Authority-side write: append one shot, return new head sequence
    fn push(&mut self, item: T) -> u64 {
        let seq = self.head_seq;
        let idx = self.idx(seq);
        self.buf[idx] = item;
        self.head_seq = seq + 1;
        self.head_seq
    }

    /// Iterate items in [start_seq, head_seq)
    fn iter_from<'a>(&'a self, start_seq: u64) -> impl Iterator<Item=(u64, &'a T)> {
        let end = self.head_seq;
        (start_seq..end).map(move |seq| (seq, &self.buf[self.idx(seq)]))
    }
}

// --- Authority (server/state-authority) usage ---
fn fire_weapon<const N: usize>(
    ring: &mut RingBuffer<ProjectileData, N>,
    origin: Vec3, dir: Vec3, tick: u32
) -> u64 {
    let data = ProjectileData { fire_tick: tick, origin, dir, hit_pos: None, finish_tick: None };
    ring.push(data) // Fusion would replicate the changed slot + head_seq
}

// --- Client usage: keep a cursor ("last seen sequence") and catch up ---
struct ClientCursor { next_seq: u64 } // next sequence the client expects to render
impl ClientCursor {
    fn new() -> Self { Self { next_seq: 0 } }

    fn catch_up<const N: usize>(&mut self, ring: &RingBuffer<ProjectileData, N>) {
        // If we fell behind by more than N, some entries were overwritten—skip to oldest available.
        if ring.head_seq.saturating_sub(self.next_seq) > N as u64 {
            self.next_seq = ring.head_seq - N as u64;
        }
        for (_seq, proj) in ring.iter_from(self.next_seq) {
            // spawn/update local visual using `proj`
        }
        self.next_seq = ring.head_seq; // mark all processed
    }
}


Do clients track “last seen sequence” or “last seen index”?
Track a sequence number (monotonic count), not just the index. The index wraps (seq % capacity), so keeping only an index becomes ambiguous after wrap‑around. Fusion’s sample mirrors this: fireCount (authoritative sequence) and visibleFireCount (client cursor) with index computed as % capacity.
Photon Engine

Practical tips

Keep ProjectileData sparse. Store the minimal fire data (origin, direction/velocity, seed/tick, maybe a hit point) and reconstruct visuals locally. Photon recommends sparse projectile data for bandwidth.
Photon Engine

Pick capacity from fire‑rate & network jitter. Capacity ≈ (max shots/sec) × (worst‑case “catch‑up” seconds) × safety factor.

Late joiners / rewind. Because the buffer is part of replicated state, late joiners receive the most recent N entries and can reconstruct recent shots. Photon’s helpers also show ring‑buffer patterns aimed at late joiners or loss‑less syncing of small items.
Photon Engine

If you want, I can adapt this sketch to your exact projectile fields (hitscan vs. kinematic) and show how to write/read two times per shot for kinematic projectiles (spawn + finish), which is also how the Fusion samples do it.
- Server spawns C
- Client gets C' and spawns P' (predicted context) and spawns A'
- Add ActionOfWrapper(C') on A' and replicates to server
- server spawns A with ActionOf(C)

Redirect input:
- ActionOfWrapper should contain metadata that tells us if predicted or not.
- ActionOfWrapper gets replicated to other clients
  - ActionOfWrapper gets mapped to C''
  - thanks to the metadata, the other client will attach the actions on the predicted entity for these other clients
  (input replication is only useful for prediction anyway)
  - the client C'' maintains the mapping between their Action A'' entity and the servers' A'

Projectile Ring buffer:
- when you press Fire, you add in the buffer a projectile metadata.
- there is a separate non-networked component that tracks the index in the buffer.
If it's behind, it pops from the buffer so spawn projectiles.


# Current bugs:

- with input-delay, we receive remote inputs at a different tick than when the client sent them!

- Something causes a full redraw of gizmos on rollbacks! What?

- Maybe my problem is that I don't recompute the VisualInterpolation previous value
  during rollbacks? So the current_value of VisualInterpolation is updated thanks to Correction, but the previous value
  is wrong!


### DON'T ENABLE GUI ON SERVER - all these errors seem related
- Is the sync system even working? C1 is 7 ticks behind the server and stays behind!

- C2 can suddenly receive a TON of remote input messages in one tick. Maybe because we were out of sync? Right after that I notice there is a SyncEvent.
  - and for an extended period of time, no remote inputs are sent.
  - the server is on tick 800, and is broadcasting remote inputs for tick ~806, but suddently we are sending 60 messages in one frame for ticks 800-860,
    which shouldn't be possible? And then for the next few ticks we are not sending any remote inputs. Then afterwards the clients are too ahead of the server,
    they send inuts for tick +30 instead of +6.
  - aeronet say we receive 56 packets!
  - on the C1, it says we received 30 packets (via aeronet) at tick 249, but on the server logs I don't see that.
  - it looks like it's because there's a 1 second period where the frame does not advance.
  - Probably due to alt-tabbing on the server or something? Anyway it is fixed by doing a headless server. But this means that

- on C1, massive rollback from state, with 80 tick rollback! Instead of actually rolling back, we should just not accept it.
  I.e. if the number of rollback ticks is too big, just ignore.
  -> this was due to the above issue I think.

- a lot of ticks can pass where we don't receive any input message -> also due to previous issue

- the remote message's end_tick can be in the future compare to our own end_tick. That seems BAD.
  - for some reason it seems like C2 could be too slow and never catch up to C1?
  - why are they so not time-synced?



# Why do my predictions don't work at all

- when i receive an update from the server for the predicted entity, i should also have received that inputs for that entity since we send
  the inputs more frequently. But it seems like it's not the case?
- Maybe we should rollback immediately when I notice that the inputs are not correct on the predicted entity?
  No need to wait to wait for an actual replication update! i.e. we should predict the ActionState! We just don't revert to the Confirmed tick, instead we revert to the history we had for that tick.

- BIG BUG: after Correction, it looks like we are not setting the inputs correctly! We look at the current input instead of looking at the current input w.r.t to the buffer!


# CHANGES FOR REPLICATION

- SinceLastAck:
  - for each group, we have an AckBevyTick, which is the bevy_tick when we received an ack for the entire group.
  - then we only send changes since that AckBevyTick.
  - issue: when there is a change, we will keep sending changes since that ack, so we potentially send
  multiple messages it the replication interval is short compared to the RTT
- SinceLastSend:
  - for each group, we have a SendBevyTick and a AckBevyTick.
  - when we send, we increment the SendBevyTick, and we will only send changes since that SendBevyTick.
  - if we have an ack, we downgrade the SendBevyTick to the AckBevyTick.
  - doesn't work in this scenario:
    - C2-tick 2: send-bevy-tick = 2
    - C1-tick 3: send-bevy-tick = 3
    - receive ACK for tick 3: ack-bevy-tick = 3
    - now we only send changes since tick 3, so if tick 2 message is lost the update will never get sent.
  - instead:
    - option 1: store the send/ack bevy ticks per (entity, component)


- Delta:
  - fix by re-enabling the Write-Delta method
  - two flavours of Delta:
    - Diffable where you compute the diff manually between two values (For example two f32)
    - A version where you already know what the diff is at each tick (new points added)

  DIFFABLE
  SinceLastAck
    - Sender:
      - need to keep track of the past component values (shared across all senders) so that we can compute the diff
        between current state and past state. Each client might have different past delta_ack_tick.
        - for example, group1-entity1: (client 1: store C-10) (client 2: store C-14)

      - when we send a message, we also include the list of delta (entity, components) that we sent in that message.
        When we have a confirmation that that message got acked for tick T:
          - we can update the delta-ticks for each of the (entity, components) (so that we know for each client what previous value to use to complete the diff)
          - we know that the client is at least at tick T. If all clients are past tick T, we can remove from the component store all values that are older than tick T.
            Or we could maintain a ref count for each past-value with the number of users that use this ack-tick as ref value. As soon as no clients don't use an ack tick, we can drop it.

      - Maybe we can use a Rc or Arc to do this?
      - The Server should keep the DeltaManager component. Or if there is no server, we keep it on the Client itself.
        - if any component gets updated and has delta-compression enabled, we update the DeltaManager of the server.
    - Receiver
      - we might receive Diff2->5 and then Diff2->9 because the server still hasn't receive the ack for 5.
        Therefore we maintain a history of past values on the client, so that we can restore the value '2' and then apply Diff2->9.

  SinceLastSend
    - Sender
      - instead of computing the diff since last-ack, we compute the diff since last-send.
        i.e. Diff2->5, then Diff5->9, etc.
      - same thing, if we have received a NACK, we compute the diff since the last ack again. However let's say:
        - send_tick = 2 -> we compute diffs from tick 2
        - Diff2->5: send_tick = 5
        - Diff5->9: send_tick = 9
        - Diff5->9: ack_tick = 9 for (entity, component)
        - Diff2->5: nack. In this case we cannot go back to ack_tick = 9! We have to go back to send_tick = 2 and send Diff2->9.
        - So the data structure we could use is a linked-list?
          We have send_ticks = 2 [ACK] -> 5 [NACK] -> 9 [ACK] and we need to go back to the first ACK that doesn't have any NACKs before.
          We only remove an element from the list if all the previous elements have been ACK. So if we had 2 [ACK] -> 5 [ACK], we can drop the 2.
      - we only remove an element from the component store if all clients have an ack-tick that is past that tick.
        This is not necessarily if all clients received a tick
    - Receiver:
      - we maintain a buffer of the updates we receive. So that if we receive Diff5->9 before Diff2->5, we wait for Diff2->5
        before applying Diff5->9.
      - on the receiver, we don't need to maintain any component history, just a buffer of the updates.
        When we are at comp for tick 2, we will always eventually receive a Diff that starts at tick 2.
      - How do we avoid waiting for Diff2->5 for too long if it's missing? Maybe we do nothing! The sender
        will eventually get a nack for Diff2->5 and resend Diff2->13 (current server tick)



# HOST-SERVER

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

