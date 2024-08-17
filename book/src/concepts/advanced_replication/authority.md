# Authority

Networked entities can be simulated on a client or on a server.
We define by 'Authority' the decision of which **peer is simulating an entity**.
The authoritative peer (client or server) is the only one that is allowed to send replication updates for an entity, and it won't accept updates from a non-authoritative peer.

Only **one peer** can be the authority over an entity at a given time.


### Benefits of distributed client-authority

Client authority means that the client is directly responsible for simulating an entity and sending 
replication updates for that entity.

Cons:
  - high exposure to cheating.
  - lower latency
Pros:
  - less CPU load on the server since the client is simulating some entities


### How it works

We have 2 components:
- `HasAuthority`: this is a marker component that you can use as a filter in queries
  to check if the current peer has authority over the entity.
  - on clients:
    - a client will not accept any replication updates from the server if it has `HasAuthority` for an entity
    - a client will send replication updates for an entity only if it has `HasAuthority` for that entity
  - on server:
    - this component is just used as an indicator for convenience, but the server can still send replication
      updates even if it doesn't have `HasAuthority` for an entity. (because it's broadcasting the updates coming
      from a client)
- `AuthorityPeer`: this component is only present on the server, and it indicates to the server which
  peer currently holds authority over an entity. (`None`, `Server` or a `Client`).
  The server will only accept replication updates for an entity if the sender matches the `AuthorityPeer`.

### Authority Transfer

On the server, you can use the `EntityCommand` `transfer_authority` to transfer the authority for an entity to a different peer.
The command is simply `commands.entity(entity).transfer_authority(new_owner)` to transfer the authority of `entity` to the `AuthorityPeer` `new_owner`.

Under the hood, authority transfers do two things:
- on the server, the transfer is applied immediately (i.e. the `HasAuthority` and `AuthorityPeer` components are updated instantly)
- than the server sends messages to clients to notify them of an authority change. Upon receiving the message, the client will add or remove the `HasAuthority` component as needed.

### Implementation details

- There could be a time where both the client and server have authority at the same time
  - server is transferring authority from itself to a client: there is a period of time where
    no peer has authority, which is ok.
  - server is transferring authority from a client to itself: there is a period of time where
    both the client and server have authority. The client's updates won't be accepted by the server because it has authority, and the server's updates won't be accepted by the client because it 
    has authority, so no updates will be applied.
    
  - server is transferring authority from client C1 to client C2:
    - if C1 receives the message first, then for a short period of time no client has authority, which is ok
    - if C2 receives the message first, then for a short period of time both clients have authority. However the `AuthorityPeer` is immediately updated on the server, so the server will only 
      accept updates from C2, and will discard the updates from C1.

- We have to be careful on the server about how updates are re-broadcasted to other clients.
If a client 1 has authority and the server broadcasts the updates to all entities, we keep the `ReplicationTarget` as `NetworkTarget::All` (it would be tedious to keep track of how the replication target needs to be updated as we change authority again), but instead **the server never sends updates to the client that has authority.**
 
- One thing that we have to be careful about is that lightyear used to only apply entity mapping on the receiver side. The reason is that the receiver receives a 'Spawn' message with the remote entity id so it knows how to map from the local to the remote id. In this case, the authority can now be transferred to the receiver. The receiver will now send replication updates, but the peer who was originally the spawner of the entity doesn't have an entity mapping. This means that the new sender (who was originally the receiver) must do the entity mapping on the send side.
  - the Entity in EntityUpdates or EntityActions can now be mapped by the sender, if there is a mapping detected in `local_to_remote` entity map
  - the entity mappers used on the send side and the receiver side are not the same anymore. To avoid possible conflicts, on the send side we flip a bit to indicate that we did a local->remote mapping so that the receiver doesn't potentially reapply a remote->local mapping. The send entity_map flips the bit, and the remote entity_map checks the bit.
  - since we are now potentially doing entity mapping on the send side, we cannot just replicate a component `&C` because we might have to update the component to do entity mapping. Therefore if the component implements `MapEntities`, we clone it first and then apply entity mapping.
    - TODO: this is potentially inefficient because it should be quite rare that the sender needs to do entity mapping (it's only if the authority over an entity was transferred). However components that contain other entities should change pretty infrequently so this clone should be ok. Still, it would be nice if we could avoid it

- We want the `Interpolated` entity to still get updated even if the client has authority over the `Confirmed` entity. To do this, we populate the `ConfirmedHistory` with the server's updates when we don't have authority, and with the client's `Confirmed` updates if we have authority. This makes sense because `Interpolated` should just interpolate between ground truth states. 


TODO:
- what to do with prepredicted?
  - client spawns an entity with PrePredicted
  - server receives it, adds Replicate
  - currently: server replicates a spawn, which will become the Confirmed entity on the client.
    - if the Spawn has entity mapping, then we're screwed! (because it maps to the client entity)
    - if the Spawn has no entity mapping, but the Components don't, we're screwed (it will be interpreted as 2 different actions)
    - sol 1: use the local entity for bookkeeping and apply entity mapping at the end for the entire action. If the action has a spawn, no mapping. (because it's a new entity)
    - sol 2: we change how PrePredicted works. It spawns a Confirmed AND a Predicted on client; and replicates the Confirmed. Then the server transfers authority to the client upon receipt.
- test with conflict (both client and server spawn entity E and replicate it to the remote) 


TODO:
- maybe let the client always accept updates from the server, even if the client has `HasAuthority`? What is the goal of disallowing the client to accept updates from the server if it has
`HasAuthority`?
- maybe include a timestamp/tick to the `ChangeAuthority` messages so that any in-flight replication updates can be handled correctly? 
  - authority changes from C1 to C2 on tick 7. All updates from C1 that are previous to tick 7 are accepted by the server. Updates after that are discarded. We receive updates from C2 as soon as it receives the `ChangeAuthority` message.
  - authority changes from C1 to S on tick 7. All updates from C1 that are previous to tick 7 are accepted by the server.
- how do we deal with `Predicted`?
  - if Confirmed has authority, we probably want to disable rollback and set the predicted state to be equal to the confirmed state?
  - ideally, if an entity is client-authoritative, then it should interact with 0 delay with the client predicted entities. But currently only the Confirmed entity would get the Authority. Would 
    we also want to sync the HasAuthority component so that it gets added to Predicted?
- maybe have an API `request_authority` where the client requests the authority? and receives a response from the server telling it if the request is accepted or not?
 Look at this page: https://docs-multiplayer.unity3d.com/netcode/current/basics/ownership/