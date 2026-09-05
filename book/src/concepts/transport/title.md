# Transport

The bottom of the stack is the IO layer: getting raw bytes from one peer to another.

The [`Link`] component is the type-erased struct that will send/receive raw bytes.
It holds a send queue and a receive queue of raw payloads, plus the link state (`Linking`, `Linked`, `Unlinked`) and 
some stats.
Lightyear systems only ever talk to the `Link`; they don't know or care how the bytes actually travel.

How the bytes travel is decided by the IO component you pair with the `Link`:

- `UdpIo` / `ServerUdpIo`: plain UDP sockets
- `WebTransportClientIo` / `WebTransportServerIo`: WebTransport (QUIC)
- `WebSocketClientIo` / `WebSocketServerIo`: WebSocket
- `CrossbeamIo`: in-memory channels, used for tests and host-server mode
- `SteamClientIo` / `SteamServerIo`: Steam sockets

So a UDP client is `Link` + `UdpIo`, a WebTransport client is `Link` + `WebTransportClientIo`, and so on. Swapping transports means swapping one component.
