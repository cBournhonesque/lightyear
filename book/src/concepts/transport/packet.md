# Packet

On top of the transport layer (which lets us send some arbitrary bytes) we have the packet layer.

A packet is a structure that contains some data and some metadata (inside the header).

## Packet header

Every Lightyear application packet begins with the fixed prefix `b"LY\x01"`: two magic bytes
(`LY`) and application wire version 1. This includes raw/P2P, Steam, Crossbeam, and application
payloads carried inside netcode. Receivers reject legacy packets and unknown versions before reading
the remaining header or decoding application data. Legacy receivers reject the new first byte as an
invalid packet type. This is a wire compatibility boundary, not a connection handshake.

The complete header is 16 bytes:

- the three-byte application wire prefix
- the packet type (one byte: data, fragment, compressed data, compressed fragment, or ACK-only)
- the packet id (a wrapping u16, two bytes)
- the last ack-ed packet id received by the sender (two bytes)
- an ack bitfield containing the ack of the last 32 packets before last_ack_packet_id (four bytes)
- the current tick (four bytes)

Multi-byte header fields use big-endian encoding. Packet-level compression leaves the entire header
uncompressed and compresses only its body. Every fragment carries its own prefixed header, and
ACK-only packets contain just this header. MTU and fragment payload calculations include all 16
header bytes.

Netcode connection packets and connect-token protocol bytes are unchanged; that protocol has its
own compatibility boundary. The application wire prefix is inside each netcode application payload,
not ahead of the netcode envelope.

## Packet data

The data will be a list of Messages that are contained in the packet.

A message is a structure that knows how to serialize/deserialize itself.

This is how we store messages into packets:

- the message get serialized into raw bytes
- if the message is over the packet limit size (roughly 1200 bytes), it gets fragmented into multiple parts
- we build a packet by iterating through the channels in order of priority, and then storing as many messages we can
  into the packet

