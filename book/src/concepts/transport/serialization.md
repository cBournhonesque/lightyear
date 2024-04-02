# Serialization

We use the bitcode library to serialize and deserialize messages. Bitcode is a very compact serialization format that
uses bit-packing (a bool will be serialized as a single bit).

When sending messages, we start by serializing the message early into a `Bytes` structure.

This allows us to:

- know the size of the message right away (which helps with packet fragmentation)
- cheaply copy the message if we need to send it multiple times (for reliable channels)
  However:
- it is much more expensive and inefficient to call `serialize` on each individual message compared with the final
  packet, and the serialized bytes compress less efficiently

## Buffers

We use a `Buffer` to serialize/deserialize messages in order to re-use memory allocations.

When we receive a packet (`&[u8]`), we create a `ReadBuffer` from it, which starts by copying the bytes into the buffer.