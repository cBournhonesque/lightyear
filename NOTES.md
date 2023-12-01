# Interesting links:

* https://medium.com/@otukof/breaking-tradition-why-rust-might-be-your-best-first-language-d10afc482ac1
  - use local executors for async, and use one process/thread per core instead of doing multi-threading (more complicated and less performant
  - one server: 1 game room per core?

PROBLEMS:
- SYNC:
  - sync only works if we send client updates every frame. Otherwise we need to take a LOT more margin on the server
    to make sure that client packets arrive on time. -> MAKE SYNC COMPATIBLE WITH CLIENT UPDATE_INTERVAL (ADD UPDATE_INTERVAL TO MARGIN?)
  - SOMETHIGN PROBABLY BREAKS BECAUSE OF THE WRAPPING OF TICK AND WRAPPED-TIME, THINK ABOUT HOW IT WORKS
- if client 1 DC and then reconnects again, we don't get a new cube.
- it looks like disconnects events are not being received?


ROUGH EDGES:
- users cannot derive traits on ComponentProtocol or MessageProtocol because we add some extra variants to those enums
- the bitcode/Bytes parts are confusing and make extra copies
- users cannot specify how they serialize messages/components
- some slightly weird stuff around the sync manager
- can have smarter speedup/down for the sync system

- Serialization:
  - have a NetworkMessage macro that all network messages must derive (Input, Message, Component)
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
  - TODO: need to handle any messages/components that contain entity handles
    - lookup bevy's entity-mapper
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

- Metrics/Logs:
  - add more metrics
  - think more about log levels. Can we enable sub-level logs via filters? for example enable all prediction logs, etc.

- Reflection: 
  - when can we use this?