# General concepts


## Network stack

There are several layers that enable lightyear to act as a games networking library.
Let's list them from the bottom up (closer to the wire):
- transport: how do you send/receive unreliable packets on the network (UDP, QUIC, etc.)
- netcode: abstraction of a connection between two peers 