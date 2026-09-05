# PacketHeader

Every packet starts with a small header (see [packet](../transport/packet.md)). It contains:

- the packet type (data vs fragment, essentially)
- the packet id (a wrapping u16)
- the last ack-ed packet id received by the sender
- an ack bitfield covering the 32 packets before that id (so 33 acks in total)
- the current tick

This schema is adopted from the GafferOnGames blogpost.
