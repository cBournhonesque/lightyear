# Packet

A packet is the byte payload that the `Link` sends through the IO backend.

Lightyear keeps packets small: the current maximum packet size is 1200 bytes. That size is chosen to avoid relying on IP fragmentation for normal UDP traffic.

Each packet has:

- a fixed-size packet header
- one or more channel batches, or one message fragment
- optional compression, depending on the packet type

## Packet types

The packet type is stored in the header. Current packet types are:

- `Data`: normal uncompressed packet containing one or more channel batches
- `DataFragment`: packet containing one fragment of a larger message
- `DataCompressed`: compressed version of `Data`
- `DataFragmentCompressed`: compressed packet wrapper for a fragment packet

Fragment payload compression is tracked separately on the fragment itself, because the whole message may be compressed before it is split into fragments.

## Normal data packet layout

A normal packet is roughly:

```text
packet header
channel id
number of messages for that channel
message 1
message 2
...
channel id
number of messages for that channel
message 1
...
```

Messages are grouped by channel because each channel has its own reliability and ordering behavior.

The packet builder tries to fill packets efficiently. Small messages can share a packet. Large messages become fragments.

## Fragment packet layout

A fragment packet carries one fragment:

```text
packet header
channel id
fragment metadata
fragment bytes
```

Fragment packets are described in more detail in the [fragmentation](../reliability/fragmentation.md) page.

## Packet ids versus message ids

Packet ids and message ids solve different problems.

Packet ids are used by the packet header acknowledgement system. They answer: "did packet 42 arrive?"

Message ids are used by reliable channels and fragmentation. They answer: "did logical message 17 arrive?" or "did fragment 3 of message 17 arrive?"

A reliable message may be sent in more than one packet over time if it needs to be retried. A fragmented message may span several packets at once.
