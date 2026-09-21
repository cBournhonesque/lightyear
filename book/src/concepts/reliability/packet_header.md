# PacketHeader

Every application transport packet starts with a 16-byte header (see [packet](../transport/packet.md)).
It contains:

- the fixed three-byte wire prefix `b"LY\x01"` (`4c 59 01` in hexadecimal)
- the packet type (data, fragment, either compressed variant, or ACK-only)
- the packet id (a wrapping u16)
- the last ack-ed packet id received by the sender
- an ack bitfield covering the 32 packets before that id (so 33 acks in total)
- the current tick

The prefix identifies application wire version 1 and is checked before any other header field,
acknowledgement, decompression, or message decoding. Legacy packets and unknown versions are rejected.
The remaining reliability fields follow the schema adopted from the GafferOnGames blogpost.
