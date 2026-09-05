# Serialization

We use postcard to serialize and deserialize messages. It's a compact, `serde`-compatible binary format (a bool takes a single byte, integers use varint encoding).

When sending messages, we start by serializing the message early into a `Bytes` structure.

This allows us to:

- know the size of the message right away (which helps with packet fragmentation)
- cheaply copy the message if we need to send it multiple times (for reliable channels)
  However:
- it is much more expensive and inefficient to call `serialize` on each individual message compared with the final
  packet, and the serialized bytes compress less efficiently

## Buffers

We use a `Writer` (backed by a reusable `BytesMut` allocation) to serialize messages, so we don't allocate from scratch for every message.

When we receive a packet, we wrap the bytes in a `Reader` (a cursor over the shared `Bytes`, no copy) and deserialize messages from it in order.
