# Concepts


There are several layers that enable lightyear to act as a games networking library.
Let's list them from the bottom up (closer to the wire):
- transport: how do you send/receive unreliable-unordered packets on the network (UDP, QUIC, etc.)
- connection: abstraction of a stateful connection between two peers
- channels/reliability: how do you add ordering and reliability to packets
- replication: how do you replicate components between the client and server
- advanced replication: prediction, interpolation, etc.
- bevy integration: 
