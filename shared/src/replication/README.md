# PER CONNECTION REPLICATER

- keep track of entity-map for this connection
- keep track of the status of replicating entities to client

# GLOBAL REPLICATER

- keep track of id of Replicate component
- keep track of the Replicate component for each entity (so we know how we replicate an entity) (so we know the
  replicate value when entity despawn)

- the enum approach for components/messages seems not good and too complicated (too many hidden derives, etc.)
- actually better if we have the protocol call explicitly using a type to register; then we can add some custom data
  associated
  with the registration
- the replicon method of having each message/component specify how can they be serialized/deserialized seems also pretty
  good

# Receiver

## spawn

- if we received a spawn but we already had the entity in the entity map, it's a bug?
- if not, spawn the entity, update the entity map
- if channel was reliable, we are good. Because the sender knows we spawned correctly
    - or do we need to send explicit confirmation?

- notify that the entity was spawned
    - that means that any components data we had received for that entity that were buffered can now be inserted
    - any components for other entities that were waiting on that entity that were buffered can now be inserted

- AI: need a entity-map, this entity map is per connection

## despawn

- if we receive a despawn without having received a previous spawn (i.e. not in entity map),
  then we keep track of it. If later on we receive a spawn, they 'cancel-out'
  (this is for when an entity is very quickly spawned-despawned, and we receive the despawn first)
- if we had received a previous spawn, just despawn the entity
    - if channel was reliable, we are good. Because the sender knows we despawned correctly

AI:

- need to keep track if the pending despawns

## insert component for E

- if E was spawned (is in entity map), then just insert the component

- If E was not spawned, then buffer that so we can insert it as soon as E is spawn
- If the component contains other entities E2, also keep track of that (we can insert it when E AND E2 are spawned)

AI:

- need an insert waitlist, and each entity they are waiting for

## remove component for E

- if the entity is not there,

## update components for E

# Sending

## spawn

- if we had previously sent a SpawnEntity message, don't do it again (i.e. entity is Spawning or Spawned on client)
  (can happen if we move Replicate and then add it again)
- spawn entity on server
- keep track that the entity is Spawning on the client (until we receive an ack)
- send SpawnEntity message

AI:

- need a structure that tracks that status of each entity on client (Spawning or Spawned)

## despawn

- send a despawn message
- despawn the entity locally

## insert component

- if entity does not exist locally (on server), abort

## removed component

We don't want to send this if there is a pending entity despawn (order systems correctly)

## send updates

- when a new client connects, we need to replicate everything that existed. Either:
    - we just send updates for all components, unreliably; and they handle it on receiver side
    - or we start by sending reliable entity-spawn/component-insert on top of the entity updates?
        - we could do both. Send all actions reliably, and all updates unreliably

- We could just
    - send each entity update. (If entity is not Spawning/Spawn on client)
    - send each component update. 


