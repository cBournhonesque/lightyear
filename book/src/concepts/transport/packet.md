# Packet

On top of the transport layer (which lets us send some arbitrary bytes) we have the packet layer.

A packet is a structure that contains some data and some metadata (inside the header).

## Packet header

The packet header will contain the same data as described in the Gaffer On Games articles:

- the packet type (single vs fragmented)
- the packet id (a wrapping u16)
- the last ack-ed packet id received by the sender
- an ack bitfield containing the ack of the last 32 packets before last_ack_packet_id
- the current tick

## Packet data

The data will be a list of Messages that are contained in the packet.

A message is a structure that knows how to serialize/deserialize itself.

This is how we store messages into packets:

- the message get serialized into raw bytes
- if the message is over the packet limit size (roughly 1200 bytes), it gets fragmented into multiple parts
- we build a packet by iterating through the channels in order of priority, and then storing as many messages we can
  into the packet

