# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
  - use local executors for async, and use one process/thread per core instead of doing multi-threading (more complicated and less performant
  - one server: 1 game room per core?

PROBLEMS:
- SYNC:
  - still doesn't work too well. Problems with pruned_rtt sometimes
  - client can be stuck in a bad state; because of rollback or sync?
- if i wait a bit on the client before sending inputs, the whole thing stays very laggy. Sync bug?
- TODO: the speedup/slowdown seems to take a lot of ticks before having any noticeable effect, which is strange.
- when the client is disconnected, the server seems to suddenly apply a bunch of inputs at once? is it because the server is behind the client?
  maybe the server should just get disconnected right away
- when there are no updates being sent, the last_received_server_tick/time is not updated very frequently, only from pings,
  in those cases the time sync manager is struggling to be super accurate, so there's a lot of speedup/slowdown
- completely breaks down when we have 2 clients! Potential causes:
  - for the first client connected, the predicted/comfirmed get completely out of sync. Which means that rollback is not working anymore?
  - for the second client, sending inputs seems to move both client cubes


ROUGH EDGES:
- users cannot derive traits on ComponentProtocol or MessageProtocol because we add some extra variants to those enums
- the bitcode/Bytes parts are confusing and make extra copies
- some slightly weird stuff around the sync manager, and we don't use the server's ping-recv-time/pong-sent-time
- can have smarter speedup/down for the sync system

- Snapshot-interpolation:
  - add a component history for server entities

- Prediction:
  - TODO: handle despawns, spawns, component insert/removes
    - despawns: add a component DESPAWN to the predicted entity (track the tick at which we add that component)
      we want the user to just be able to use `commands.despawn(entity)` without worrying about what's going on behind the scenes
      If during rollback we realize it shouldn't be despawning, we remove that component
      If latest_received_server_tick reaches the tick saved in DESPAWN, that means we won't rollback that despawn, so we actually despawn
    - component insert: 
      
  - TODO: 2 ways to create predicted entities
    - server-owned: server creates the confirmed entity, when client receives it, it creates a copy which is a predicted entity -> we have this one
    - client-owned: client creates the predicted entity. It sends a message to client, which creates the confirmed entity however it wants
      then when client receives the confirmed entity, it just updates the predicted entity to have a full mapping -> WE DONT HAVE THIS ONE YET
     
  - TODO: maybe define different 'modes' for how components of a predicted entity get copied from confirmed to predicted
    - with_rollback: create a component history and rollback to the confirmed state when needed
    - copy_once: only copy the component from confirmed to predicted once, and then never again
      - if we don't have this, the color will be reverted to the confirmed color every time we rollback
    - not_copy: never copy the component from confirmed to predicted

- Replication:
  - Fix the enable_replication flag, have a better way to enable/disable replication
  - POSSIBLE TODO: send back messages about entity-actions having been received? (we get this for free with reliable channels, but we need to notify the replication manager)

- Message Manager
  - TODO: need to handle any messages/components that contain entity handles
  - TODO: run more extensive soak test


- Packet Manager:
  - TODO: Send Keepalive as part of Payload instead of KeepAlive
    - so that we can receive ack bitfields frequently (ack bitfields needed for reliable channels not to resend)
    - DISABLE NETCODE KEEP-ALIVE AND ROLL-OUT MY OWN WITH KEEPALIVE DATA TYPE! (this works because any packet received counts as keep alive)
    - actually, don't need to disable netcode keep-alive, just send payload keep alive more frequently!
    - or just prepare an ACK response whenever we receive anything from a reliable sender? (so the reliable sender gets a quick ack bitfield)
  - TODO: Pick correct constant values for MTUs, etc.
  - TODO: construct the final Packet from Bytes without using WriteBuffer and ReadBuffer, just concat Bytes to avoid having too many copies

- Channels:
  - TODO: add channel priority with accumulation. Some channels need infinite priority though (such as pings)
  - TODO: add a tick buffer so that inputs from client arrive on the same corresponding tick in the server.
    - in general the tick buffer can be used to associate an event with a tick, and make sure it is received on the same corresponding tick in remote

- UI:
  - TODO: UI that lets us see which packets are sent at every system update?

- Reflection: 
  - when can use this?


# Tenets

* similar to naia, but tightly integrated with Bevy. No need to wade through WorldProxy, etc.
* re-uses a lot of bevy's stuff: time, change-detection, etc.