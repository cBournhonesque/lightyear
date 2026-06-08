# Packet header

Every transport packet starts with a small header. In the current implementation it is 17 bytes.

The header contains:

- packet type
- packet id
- latest packet id received from the remote
- a bitfield for the 32 packets before that latest received packet
- the sender's current Lightyear tick

This is the same basic acknowledgement strategy described in Glenn Fiedler's reliability articles: every packet carries information about recently received packets, so acknowledgements do not need their own packet.

## Packet id

Each side increments its own packet id every time it sends a packet. Packet ids are local to the sender.

The id is not the same as a message id. A packet is one network payload. A message is a logical piece of channel data. One packet can contain several messages, and one fragmented message can occupy several packets.

## Ack id and bitfield

The header says:

- the newest packet id I have received from you is `last_ack_packet_id`
- here is a 32-bit mask for the 32 packet ids before that

If bit 0 is set, the packet immediately before `last_ack_packet_id` was received. If bit 1 is set, the packet two before it was received, and so on.

That gives acknowledgements for 33 packets in every outgoing packet.

## Loss detection

When a sent packet is acknowledged, Lightyear can remove it from the "not acked yet" set.

If a sent packet stays unacknowledged for long enough, it is treated as lost. The timeout is based on the current RTT estimate, with a minimum delay to avoid marking packets lost immediately and a maximum delay to avoid holding old state forever.

Reliable channels use that information to retry messages or fragments.

## Remote tick

The header also includes the sender's current tick.

That tick is not part of reliability itself, but it is useful for higher layers. Timelines, replication checkpoints, interpolation, and prediction all need to know when remote data was produced.
