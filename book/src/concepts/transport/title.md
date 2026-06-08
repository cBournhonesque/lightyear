# Transport

The transport layer is where Lightyear turns typed messages into bytes that can move through a `Link`.

It sits above the raw IO backend and below messages, inputs, and replication:

```text
game systems
messages / inputs / replication
transport channels
packets
link / IO backend
network
```

The raw link only knows how to send and receive byte payloads. The transport adds the pieces games usually need on top of that:

- channels
- reliability
- ordering and sequencing
- packet acknowledgements
- fragmentation for large messages
- optional compression
- bandwidth/priority decisions

## Link versus transport

The `Link` layer is intentionally small. UDP, WebTransport, WebSocket, Steam, local crossbeam channels, or a test harness can all feed bytes into a `Link`.

The `Transport` component is the next layer up. It owns channel senders and receivers, a packet builder, compression settings, and packet/message acknowledgement bookkeeping.

That split is useful because reliability should not care whether bytes came from UDP or an in-process channel.

## Sending

When a system sends bytes on a channel, the transport does not immediately write to the socket. It buffers the message in that channel's sender.

Later, during the transport send set, Lightyear:

1. asks each channel sender which messages are ready
2. sorts and packs messages into packets
3. writes packet headers
4. applies compression if configured and useful
5. queues the packet bytes on the link sender

The link backend then flushes those bytes to the actual IO implementation.

## Receiving

On receive, the order is reversed:

1. the IO backend pushes bytes into the link receiver
2. the transport parses the packet header
3. packet acknowledgements are processed
4. packet payload is decompressed if needed
5. messages are routed into channel receivers
6. higher-level systems read messages, inputs, or replication payloads from those receivers

The transport also emits a `PacketReceived` event with the remote tick from the packet header. Timeline and replication systems use that timing information.

## System sets

The transport runs in two main sets:

- `TransportSystems::Receive` in `PreUpdate`, after `LinkSystems::Receive`
- `TransportSystems::Send` in `PostUpdate`, before `LinkSystems::Send`

That schedule gives higher layers a simple shape:

- bytes are received from the link
- transport turns packets into channel messages
- messages, inputs, and replication are processed
- gameplay runs
- outgoing messages are serialized and buffered
- transport builds packets
- the link flushes bytes to the IO backend

You usually do not need to put gameplay systems inside transport sets. They are mainly useful when you are writing a custom integration layer that needs to observe raw channel traffic or packet events.

## Implementations

The transport is backend-agnostic. The concrete IO layer can be:

- UDP sockets
- WebTransport
- WebSocket
- Steam
- local crossbeam channels for tests and host-style setups

The same channels and packet logic sit above all of them.
