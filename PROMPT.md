# Replication Groups

I am building a networking library to replicate ECS data over the network.
I introduced the concept of a ReplicationGroup to:
- let the user easily configure common replication behavior that is shared between multiple entities (for example the priority, or replication interval)
- be able to specify that some entities should be replicated in the same message. There is some nuance there:
  - for entities that are linked by a relationship: we mostly want the entities to be spawned together (otherwise if we receive the child first we would get an error) but updates could be received in multiple messages
  - for prediction/rollback, I found it simpler to make sure that all prediction entities are exactly on the same tick. By guaranteeing that all updates are sent as part of the same message, I can make sure that for a given tick I know the exact state of all predicted entities
  - for other cases, sending all updates together just adds extra overhead for no benefit


My code related to ReplicationGroups is not as efficient as it could be for multiple reasons:
- currently a ReplicationGroup is a u64 (as it could be based on an entity), which can be expensive to serialize
- I buffer all updates in a double hashmap (group_channels -> pending_updates) which can be expensive to update
- on the sender/receiver side, I include information about the group, which adds even more overhead (hashmaps on the receiver side, etc.)

Also the API can be confusing:
- ReplicationGroup is a component added on the replicated entity, but multiple entities could have a ReplicationGroup component with the same entity but different settings (send_frequency, should_send, etc.)


I would like you to give me a detailed plan of what I can do to improve the situation.
Here are some ideas I had:
- wwe could have a ReplicationGroup entity that tracks the common replication settings for a whole group of entities
(we don't have to use entities if you think there is a better approach, like storing in a resource)

- each entity belongs to exactly one ReplicationGroup, this could be symbolized using a relationship. Or the Replicate component could directly contain the ReplicationGroupId.
By default the ReplicationGroupId would be 0 (meaning that there is no ReplicationGroup entity and we just use the default setting), or it could be an Entity: the ReplicationGroup entity.

- When we spawn a child of an entity, by default we insert ReplicateLike (meaning that the child is replicating with the same params as the parent). But we would also need to put the parent in a group
that has Consistency::Spawns so that the whole hierarchy is replicated together. In that case we could simply add a ReplicationGroup component on the root entity (i.e. it becomes a replication group!)
But what happens if that entity later becomes the child of another entity? Or if there are two different hierarchies using two relationship components?

- the ReplicationGroup would have an extra setting:
```
pub enum GroupConsistency {
  // all updates for all entities in this group are replicated in the SAME message
  All,
  // all spawns for all entities in this group are replicated in the SAME message
  Spawns,
  // updates are not guaranteed to be replicated in the same message
  None,
}
```
this setting would have to be replicated to the receiver, so it knows how to interpret the replication messages.

- For groups that have `GroupBehavior::None`: we can use the writer of the ReplicationSender.
- the ReplicationGroup component could contain the Writer that is used to send messages. If `should_send=True` and it's time to send an update, the buffer system could write the actions/updates directly in its associated writer. If there were any Actions it would send the whole message as an EntityActionsMessage.

- Instead of iterating through entities in the buffer system, we should iterate through groups, that way we fetch the group metadata only once, and we can skip some groups early.

- For these groups, can we omit the hashmap `group_channels` on the ReplicationReceiver side?

Are there any other kind of changes you're suggesting?


Let's walk through some examples:
- server spawns some unrelated entities. They don't have a ReplicationGroupChildOf component
  We iterate





## Solution
- ReplicationGroup: settings that are common for a whole group of entities (replication_frequency, consistency)
-


# Optimize replication

Currently when we insert Replicate, we add the list of senders concerned by an entity using an EntityHashMap in Replicate.
And then we add the entity to the sender's `replicated_entities`.
That's a lot of data duplication.

What if we:
- created a mapping from the entity holding a ReplicationSender to a integer, so that the list of all senders can be stored in a FixedBitSet. (when a client connects, we give them the newest integer. When a client disconnects, do we compact the bitset? probably not because we would need to update the bitset inside each Replicate...)
  - when a sender connects: we associate it with a free bit (by walking through the bitset until the first free one). Maybe we can also
  - when a sender disconnects: we update the bit mapping (that bit becomes free).
- for each entity that has Replicate:
  - we store a bitset that contains the list of all senders that are interested in this entity.
  - then when we buffer updates, we go through the list of entities that have Replicate (stored once somewhere), and check the bitset (or the hashmap) to check if this entity is of interest.
  - the hashmap would also store if we we have authority or not for that entity.
    (do we need to distinguish if Replicate::to_client_1 = True but we have no authority?)
    Maybe in this scenario:
    - we receive an entity via replication, so we have no authority.
    - if we add Replicate on the entity, we should know that we have no authority over the entity.
    Thus we need two bitvecs on `Replicate`. One for authority and one for replication.
  - on the receive side, we can check that bitset to see if we need to worry about it.

Try to implement this idea:
- replace Replicate.senders with two bitvecs (one for authority and one for relevance (i.e. we should be replicating))
- on ReplicationSender addition: update the PeerMetadata resource to have the mapping between the Entity and the free bit.
- on ReplicationSender removal: update the bitmap.
- on disconnection: update `handle_disconnection` to remove that bit from all Replicate and CachedReplicate
- on connection/Replicate insert: update `Replicate`/`CachedReplicate` to have that bit
- on Replicate add: update some global IndexMap containing the entity with entities that should be considered.


We can use the smallbitvec crate for this.


Alternative:
- use a EntityHashMap inside Replicate with ReplicationState {
  Gained,
  Lost,
  Maintained,
  NoAuthority, // we don't have authority, even if visibility is provided
  Always, // we have authority and we are always visible
}

visibility: Visibility {
  Gained,
  Lost,
  Maintained,
  Always
}
authority: Authority {
  Yes,
  No, -> this means that
  Unknown
}

- the thing is we want to know at receive time if an entity definitely has no authority for a given sender.
Example: client sends E1 to server, who adds replicate. Replicate should definitely not have authority.
We need an extra component for that.
- it's ok to have a single component for visibility, because we will iterate through them in `buffer` every frame.

- Replicate::lose_visibility() or Replicate::gain_visibility() activate visibility.


So:
- visibility is part of Replicate.
  - update visibility system
- unique list of Replicate entities to use
- separate authority component

Maybe we can use a bitset with multiple bits per sender to represent authority/visibility/etc.

Could add a single ReplicationState component that contains both visibility and authority.
It's separate from Replicate so users wouldn't overwrite it. + we can add the "non-authoritative" information
independently from the user adding Replicate.
 

CachedReplicate:
- if we change Replicate, we want to only send spawn to new senders, and despawn to the diff of senders.

With separate ReplicateStatus:
- replicated to 1, 2
- spawned to 1, 2
- insert Replicate to 1, 3
-  -> add sender to ReplicateStatus for 3
-  -> since we spawned on 2, prepare a despawn for 2.
no need for cached replicate!

- if we add PredictionTarget before Replicate
  - we add to ReplicateState but not to ReplicateEntities
  - we add 'prediction' but not 'authority' (authority is unknown)


# Authority
- make it as optional as possible if feature is not active

# Mode

The thing is that we don't want to penalize cases where there is only one ReplicationSender in the world:
- single-client
The cases with multiple senders could be:
- server
- P2P with distributed authority.

We could add features:
- mode_server: potentially multiple ReplicationSender
- mode_p2p:
- mode_client:


MODES
# Client
- each replicate can only have a single sender (no need for a hashmap)
# Server 
- 
# P2P-Deterministic
# P2P-StateReplication
- multiple links, no state replication?
# HostClient
- Server, but one of the clients is the host


- LightyearMetadata resource 
  - includes the Mode (Client, Server, P2P)
    - Other potential modes:
      - maybe a client wants to connect to multiple servers in a server-mesh scenario?
      - i don't think we would ever want multiple servers in the same app
    - with maybe a pointer to the 'main' entity?
  - includes the TickDuration.
  - maybe includes the LocalTimeline!

- Client mode:
  - Replicate/Authority/NetworkVisibility don't need hashmaps
- 


# Component visibility

It would be helpful if you could precompute a set of override rules for visibility.
For example we have ComponentReplicationOverride that is global or per-sender.

What if you specified once a set of ComponentReplicationOverride rules.
Then you can attach a Rule1 on a Sender and on an entity. This means that the rule applies to both.


# Multi-Server

I think it's too complicated to properly handle multi-servers.

We can have either:
- multiple 'servers' that are still part of a same global server
  - i.e. global server timeline, multiple Server entities (Websocket, WebTransport, etc.) that each have their own ClientOfs.
  - but otherwise the big 'SERVER' is the same. (it is a resource)
  - the server is just there to manage multiple client connections, but otherwise the app itself only has a single server
- single client
- host-server: one of the clients is also a ClientOf of one of the servers
- P2P: multiple Links in the same app?
  - these are not Clients though, because Clients implies that there is a Server on the other side.
- deterministic via server relay: 
  - clients have a single Link
  - we could again have multiple servers, but a single SERVER timeline

We can have a component Role that indicates that the this is the main timeline.
(or AuthorityBroker).

