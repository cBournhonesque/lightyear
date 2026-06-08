# Serialization

Serialization is the boundary between Rust types and network bytes.

Most user-facing data starts as a typed message, input, or component. Before it can be sent over a transport channel, Lightyear serializes it into bytes. The transport then treats those bytes as a logical message.

## Why serialize early?

Lightyear serializes messages before packet packing.

That has a few benefits:

- the transport knows the byte length before choosing a packet
- reliable channels can store the serialized bytes for retries
- fragmentation can split the bytes without knowing the original Rust type
- message bytes can be cheaply cloned with `Bytes`

The tradeoff is that serializing each message independently can be less compact than serializing a whole packet at once. For a game networking library, the bookkeeping benefits are usually worth it.

## Entity mapping

Serialization alone is not enough for data that contains `Entity`.

The server and client do not share Bevy entity ids. If a replicated component or message contains an entity reference, it has to be mapped between worlds.

Use Bevy's `MapEntities` support for those types:

```rust,ignore
#[derive(Serialize, Deserialize, Clone)]
pub struct AttachTo {
    pub parent: Entity,
}

impl MapEntities for AttachTo {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.parent = mapper.get_mapped(self.parent);
    }
}
```

If you forget this, the receiver can end up with an entity id that points to the wrong local entity, or to no entity at all.

## Buffers

The transport uses reusable buffers for packet read/write work where possible. On receive, packet bytes are parsed into subslices so channel receivers can keep message bytes without copying the whole packet repeatedly.

This matters because high-frequency networking code spends a surprising amount of time allocating if you let it.

## Compression

Compression is optional. It can be enabled on the `Transport` through `CompressionConfig`.

Compression is most useful for larger structured payloads. It is usually not helpful for tiny high-frequency messages, and it can be actively harmful if it burns CPU for a handful of bytes.

Lightyear can compress packet payloads and can also compress large message payloads before fragmentation when that is useful.
