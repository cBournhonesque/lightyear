# Transport

The bottom of the stack is the IO layer: getting raw bytes from one peer to another.

The [`Link`] component is the type-erased struct that will send/receive raw bytes.
It holds a send queue and a receive queue of raw payloads, plus the link state (`Linking`, `Linked`, `Unlinked`) and 
some stats.
Lightyear systems only ever talk to the `Link`; they don't know or care how the bytes actually travel.

How the bytes travel is decided by the IO component you pair with the `Link`:

- `UdpIo` / `UdpEndpoint`: plain UDP sockets
- `WebTransportClientIo` / `WebTransportEndpoint`: WebTransport (QUIC)
- `WebSocketClientIo` / `WebSocketEndpoint`: WebSocket
- `CrossbeamIo`: in-memory channels, used for tests and host-server mode
- `SteamClientIo` / `SteamEndpoint`: Steam sockets

So a UDP client is `Link` + `UdpIo`, a WebTransport client is `Link` + `WebTransportClientIo`, and so on. Swapping transports means swapping one component.

An accepting transport component requires `Endpoint`, which owns the collection of per-peer
`Link` entities through `LinkOf { endpoint }`. It does not imply the `Server` role: a P2P peer
can also own an endpoint. Trigger `LinkStart` to open its listener.

Native accepting endpoints are available with either the transport crate's `p2p` or `server`
feature. `lightyear/p2p` forwards `p2p` to whichever optional transports you enable, so a peer
can listen without enabling Lightyear's `server` feature. For example, select
`default-features = false` and `features = ["std", "p2p", "udp"]` to use
`lightyear_udp::endpoint::{UdpEndpoint, UdpEndpointPlugin}` without server plugins.
For WebSocket, WebTransport, and Steam, the transport's `p2p` feature enables Aeronet's
accepting-side support (called `server` by Aeronet), not Lightyear's authority role.

When installing an endpoint transport plugin directly, it also installs the shared endpoint
lifecycle support: unlinking an endpoint unlinks and despawns its child links, and each new child
inherits the endpoint's receive conditioner. Client-only builds retain the shared `Endpoint`,
`LinkOf`, and Aeronet bridge types; accepting transport components require `p2p` or `server`.

For an authoritative server, add `Server` alongside the endpoint component and the appropriate
connection component. `ServerUdpIo` is the UDP shorthand that requires both `UdpEndpoint` and
`Server`. Configure inherited receive conditioning with `Endpoint::new(conditioner)` rather
than on `Server`; access the fan-out collection through `Endpoint`.
