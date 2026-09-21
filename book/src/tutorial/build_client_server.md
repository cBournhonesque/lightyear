# Setting up the client and server

The client and server will both be bevy Entities to which you can add components to customize their networking behaviour.
Here are some of the common components:
- [`Link`](https://docs.rs/lightyear/latest/lightyear/link/struct.Link.html) represents an IO link between a local peer and a remote peer that can be used to send and receive raw bytes
- [`Transport`](https://docs.rs/lightyear/latest/lightyear/prelude/struct.Transport.html) adds the capability of setting up various Channels that each provide different reliability/ordering guarantees for a group of bytes
- [`MessageManager`](https://docs.rs/lightyear/latest/lightyear/prelude/struct.MessageManager.html), [`MessageSender<M>`](https://docs.rs/lightyear/latest/lightyear/prelude/struct.MessageSender.html), [`MessageReceiver<M>`](https://docs.rs/lightyear/latest/lightyear/prelude/struct.MessageReceiver.html) are used to send and receive messages over the network.
  A message is any rust type that can be serialized/deserialize into raw bytes.
- [`ReplicationSender`](https://docs.rs/lightyear/latest/lightyear/prelude/struct.ReplicationSender.html) is added to a link entity to enable sending replicated entities and components through that connection, and [`ReplicationReceiver`](https://docs.rs/lightyear/latest/lightyear/prelude/struct.ReplicationReceiver.html) is added to receive them.

## Link

The [`Link`] component is the primary component that represents a connection between two peers. Every network connection is represented by a link. On the server side, you have a [`Server`](https://docs.rs/lightyear/latest/lightyear/link/prelude/struct.Server.html) 
component which spawns a new entity with a [`Link`] component every time a new client connects to it. The [`LinkOf`](https://docs.rs/lightyear/latest/lightyear/link/prelude/struct.LinkOf.html) relationship component is added on these entities to help you identify the 
[`Server`] that they are connected to.

The link is agnostic to the actual io layer, you will have to pair it with an actual io component (`UdpIo`, `WebTransportIo`, etc.) to start sending and receiving bytes.

## Connection

Lightyear makes a distinction between a [`Link`] and a `Connection`.
A `Link` is a low-level component that represents a raw IO link, which can be used to send and receive bytes.
A connection adds resolved `LocalId` and `RemoteId` routing identities to a link.
Transports use the default connection backend when no explicit backend is selected: no extra
Raw marker or plugin is required for debugging. Add `NoDefaultConnection` when using a transport
only for bytes, without connection admission.

The default backend derives IDs from native transport facts and preserves explicitly configured
debug IDs. It does not negotiate those IDs or authenticate the peer. NAT and local-only transports
can give the two ends different routing labels. Use netcode when the client and server need an
authenticated, agreed identity; use application-level player IDs for persistent accounts.

Steam's normal `SteamPlugin` combines `SteamTransportPlugin` and `SteamConnectionPlugin`.
Use the transport half alone when composing it with another explicit connection backend.

The lifecycle of a connection is controlled by several sets of components.

You can trigger [`Connect`] to start the connection, and [`Disconnect`] to stop it.

The [`Disconnected`], [`Connecting`], [`Connected`] components represent the current state of the connection.

On the server, [`Start`] and [`Stop`] components are used to control the server's listening state.
The [`Stopped`], [`Starting`], [`Started`] components represent the current state of the connection.

While a client is disconnected, you can update its configuration (`ReplicationSender`, `MessageManager`, etc.), it will be applied on the next connection attempt.

Configure one connection backend before connecting. Explicit backend components require
`NoDefaultConnection` to suppress raw/default admission; without a backend or opt-out,
supported transports use the default path. Competing backends and live backend switching
are unsupported, not resolved by ownership or conflict tracking.

Remote peer IDs are unique across all endpoints in the same Bevy `World`. A backend registers the
final `LocalId` and `RemoteId` before adding `Connected`, so connection observers can immediately route to it.
Authenticated, P2P, and explicitly requested duplicate IDs are rejected without notifying
`Connected` observers. An unrequested default/debug ID may select a free local alias after a
collision; it never renames the incumbent or establishes cross-peer agreement. Normal disconnect
or despawn releases only the binding owned by that entity.

For netcode, a preinserted `LocalId` is an expectation, not an override of the token. Manual
authentication can validate it before the handshake. Opaque tokens keep that ID encrypted, so their
expected ID is checked after authentication but before `Connected`.

Starting a `Server` endpoint also reserves the World-wide `PeerId::SERVER` route for that
endpoint. A second independent local server endpoint, or an outgoing remote-server route,
cannot replace that owner: it fails explicitly. Separate Worlds or explicit P2P roles are
needed where independent server roles would be ambiguous; see [routing scope](../concepts/connection/multi_connection.md).

### Numeric routing aliases

`PeerId(pub u64)` is opaque. `PeerId::SERVER` is zero, reserved for the appropriate side of an
explicit client/server connection; ordinary peers use `1..=u64::MAX`. Netcode token client IDs and
Steam IDs retain their numeric values. Applications and token minters using client ID zero must
migrate; it is rejected, not silently remapped. Host clients also use nonzero aliases: identify
their role with `HostClient`, never with a particular number.

The former enum variants and `to_bits()`/`is_local()` methods are removed. Use `.0` for the numeric
alias and transport-owned components for native identities, addresses, and display. Do not decode
an alias into a dial target or use it as a persistent account/player ID. Replicon `NetworkId` uses
the final admitted numeric alias directly.

These source/API changes are separate from the application-wire change below and the
[netcode token compatibility boundary](#minter-format-and-compatibility). Remove the former
Raw crate/markers and shared `LocalAddr`/`PeerAddr` configuration; use the native transport
components in [the dialing recipes](#native-dialing-and-failover). There are no compatibility
aliases or automatic conversions for old identity variants.

Socket projections use `peer_id_from_socket` with domain `b"udp"`, `b"websocket"`, or
`b"webtransport"`. The canonical input is family byte 4 or 6, IP octets, big-endian port, and for
IPv6 a big-endian scope ID. IPv4-mapped IPv6 normalizes to IPv4; flow labels are excluded.
`peer_id_from_native` hashes `b"lightyear-peer-id-v1\0"`, the domain length as a big-endian u64,
the domain, and canonical identity bytes using SHA-256. Its first eight bytes form a big-endian
alias; zero maps to one. For example, UDP `127.0.0.1:4100` projects to `0xd1c76592148b73b6`.
Retain the full native identity; a projection is neither authentication nor collision agreement.

This is an incompatible application wire change. Every Lightyear network application packet now
starts with `4c 59 01`, including non-netcode/P2P traffic; old headers are rejected before message
decoding. `PeerId::ToBytes` is one big-endian u64 without a variant tag; Serde represents the
underlying u64 using the selected serializer's integer encoding. Upgrade all peers together.
The existing socket-mode netcode token encoding is unchanged.

### P2P discovery admission

A lobby endpoint resolves one self-ID before admitting child links. To choose it, insert `LocalId`
before starting the native endpoint; every child inherits that identity. Conflicting endpoint or
child requests fail instead of silently choosing another ID.

UDP, WebSocket, and WebTransport lobby links exchange their existing reliable `LobbyAnnounce`
before becoming `Connected`. While pending, a link exposes neither public ID nor a routing-map
entry. Only the bounded, own-only announcement is allowed through the typed-message layer;
ordinary application messages wait for admission. Accepted stream source ports never become
provisional public identities. Bare default connections without lobby discovery add no handshake.

Publish `Lobby::local()` together with `Lobby::local_dial_context()` in an invite, and seed the
receiver with `Lobby::add_bootstrap_with_context`. The context contains native dialing facts,
including the WebTransport certificate digest, independently of the chosen routing alias. Treat
it as transport-owned bytes, not as stable identity material or proof of an account.

An admitted link cannot announce a different sender ID. Duplicate claims are rejected, and
third-party discovery cannot replace an established direct binding. Rejection does not guarantee
that all peers choose the same claimant: `P2PStart` still requires admitted candidates and the
existing shared-roster agreement barrier before gameplay.

Native authentication does not authorize an advertised alias or account. For example, a valid
WebTransport pin proves possession of the pinned certificate, but the application must trust
the association between that pin, the intended peer, and its lobby/account permissions.
An untrusted discovery announcement cannot supply that authorization merely by including a pin.

### Gameplay identity and connection replacement

Keep durable player/account identity in application-owned state, and associate it with the
current admitted connection after the application's authentication and authorization checks.
`PeerId` is a routing alias, not a saved-game key, account credential, or reconnect guarantee.
The `lobby` example carries `LobbyClientId` onto a newly spawned game connection when leaving
the lobby. Switching from netcode to the in-process host backend can change the routing alias
without changing that application identity; the example ID itself is not an account credential.

Do not remove one backend marker and insert another on the same entity. Disconnect/despawn the
old entity to release its route, create the replacement with its backend and native target,
then associate its final admitted alias with the same player. Host migration must use
`HostClient` to identify the local role and nonzero ordinary IDs, not the old zero-ID convention.
Neither replacement nor [P2P admission](#p2p-discovery-admission) negotiates a new shared identity
or renames an existing peer.


## Client

A client is simply an entity with a [`Link`] to which the [`Client`] marker component is added.
The marker component is used in conjunction with the protocol to customize the behaviour of the link entity.
For example if a message is added to the protocol with
```rust,noplayground
app.register_message::<Message1>()
  .add_direction(NetworkDirection::ServerToClient);
```
then a `MessageReceiver<Message1>` component will automatically be added to any `Client` entity.

You can also just add the [`MessageReceiver<M>`] component directly to the client entity to receive messages of type `M` from the server.

Here is how you can set up a client in your app:

```rust,ignore
let auth = Authentication::Manual {
    server_addr: SERVER_ADDR,
    client_id: 1,
    private_key: Key::default(),
    protocol_id: 0,
};
let client = commands
    .spawn((
        Client,
        Link::default(),
        ReplicationReceiver,
        NetcodeClient::new(auth, NetcodeConfig::default())?,
        UdpIo::new(CLIENT_ADDR, SERVER_ADDR),
    ))
    .id();
commands.trigger(Connect { entity: client });
```

Let's walk through this:
- we add the [`Client`] marker component to the entity to identify it as a client.
- UDP's constructor owns its bind and dial addresses; no shared address components are required.
- we add the [`Link`] component to the entity, which will be used to send and receive raw bytes over the network.
- we add the [`ReplicationReceiver`] component to the entity, which will be used to receive replicated entities and components from the server.
- netcode is the explicit connection backend in this example. For testing, `Manual` authentication
  specifies the server address and client ID. Omitting `NetcodeClient` uses the unauthenticated default.
- finally we add the [`UdpIo`] component to the entity, which will be used to send and receive UDP packets over the network.

Finally we trigger the [`Connect`] trigger to start the connection process.

(The examples wrap this setup in `ExampleClient`/`ExampleServer` helpers in `examples/common`, but the components are the same ones you see here.)


## Server

Similarly, a server is an entity to which the [`Server`] marker component is added.
Everytime a new io link is established with a remote peer,
a new entity will be spawned with the [`LinkOf`] component that will mark that [`Link`] as being a child of the endpoint owned by the [`Server`].

```rust,ignore
let server = commands
    .spawn((
        NetcodeServer::new(NetcodeConfig::socket([SERVER_ADDR])),
        UdpEndpoint::new(SERVER_ADDR),
        Server,
    ))
    .id();
commands.trigger(Start { entity: server });
```

`NetcodeServer` selects the authenticated connection backend and adds the `Server` role.
Its `ServerTokenBinding::Socket` addresses authorize incoming tokens; they are not a transport
bind configuration. An empty list rejects connection requests. This also applies to Crossbeam:
configure the same authorization address used when minting the token, without inventing socket
components on the channel link.

`UdpEndpoint::new` owns the actual UDP bind. After a port-zero bind, read `UdpLocalAddr` for the
assigned port. Choose the publicly reachable address for that port, call
`NetcodeServer::set_expected_addresses([advertised_address])`, and mint tokens using that address
before clients connect. NAT/public addresses can differ from the listener's bind address.

For local development, wildcard and loopback addresses of the same family and port remain
equivalent, such as `0.0.0.0:5000` and `127.0.0.1:5000`. There is no implicit address-validation
opt-out for addressless transports.

Finally we trigger the [`Start`] trigger so that the server can start listening for incoming connections.

## Native dialing and failover

### UDP and stream targets

For a single UDP peer, use `UdpIo::new(bind_addr, peer_addr)`; for an accepting endpoint,
use `UdpEndpoint::new(bind_addr)`. The requested `bind_addr` remains configuration, including
port zero. After binding, read `UdpLocalAddr` for the actual socket and `UdpPeerAddr` for a
link's native remote route. Do not infer either address from a `PeerId`.

WebSocket clients use `WebSocketClientIo::from_url(config, "wss://game.example/session")` or
`from_host(config, WebSocketScheme::Secure, "game.example", 443)`. WebTransport clients use
`WebTransportClientIo::from_url("https://game.example/session", certificate_digest)` or
`from_host("game.example", 443, certificate_digest)`. Hostnames stay intact for DNS, TLS and browser
APIs; do not pre-resolve them into IP addresses. The host constructors also handle unbracketed IPv6.

WebSocket's `WebSocketEndpoint { config }` uses Aeronet `ServerConfig` for the requested bind.
`WebTransportEndpoint { bind_addr, certificate }` owns its requested bind and TLS identity.
Read `WebSocketLocalAddr` / `WebTransportLocalAddr` after binding for the actual listener address,
including the assigned port after requesting zero. `WebSocketPeerAddr` / `WebTransportPeerAddr`
describe observed native sessions, not dial inputs or advertised public addresses.
Browsers do not expose socket addresses, so those observations may be absent; never fill them
with fake addresses.

A WT certificate digest is a hexadecimal SHA-256 pin obtained through a trusted channel, not
an arbitrary value copied from an untrusted announcement. Use the intended server's trusted
digest with `WebTransportClientIo`; native TLS and [netcode authorization](#identity-bound-netcode-tokens)
are independent. Empty pins are not a production pinning recipe: with dangerous configuration,
an empty native pin disables certificate validation.

### Steam native targets and facts

Use `SteamClientIo { target, config: SessionConfig::default() }`, where `target` is
`ConnectTarget::Addr(server_addr)` for socket dialing or
`ConnectTarget::Peer { steam_id, virtual_port }` for a Steam relay peer. An accepting
`SteamEndpoint { target, config: SessionConfig::default() }` takes `ListenTarget::Addr(bind_addr)`
or `ListenTarget::Peer { virtual_port }`. Retain the full `SteamId` in the native target,
not a reverse conversion from a generic alias. Initialize Steam resources before Lightyear
plugins with `SteamAppExt::add_steam_resources(app_id)`; this requires a running Steam client.

Once a session is established, `SteamSessionFacts` exposes the native `local` identity and
optional authenticated `remote` identity. Outgoing socket sessions have `remote: None` because
Aeronet does not expose it; their original address remains in `SteamClientIo::target`.
The Steam backend uses the server role alias for those client/server links. Address-target
P2P sessions cannot authenticate an alias and are rejected. Use `SteamPlugin` for native Steam
admission, or its transport half with another explicit backend—not two admission backends
on one entity. This does not establish an end-to-end Steam identity-token pairing.

### Crossbeam without sockets

Call `CrossbeamIo::new_pair()` and insert one result on each link entity, normally in separate
client/server Worlds. The server mirror has `LinkOf { endpoint: server }`; start it with
`LinkStart`, and trigger `Connect` on the `Client` entity at the other end. Do not insert
`Linked` manually. Channel links have no bind or dial socket, and their default entity-derived
IDs are local debug labels, not an agreement between the two Worlds. If a test requires agreed
IDs, explicitly coordinate `LocalId` / `RemoteId` or use an explicit connection backend.
For socket-mode netcode, the [configured authorization address](#server) belongs to the token,
not the channel. `HostClient` is a distinct in-process backend, not Crossbeam.

### Native debug display

Display the numeric `LocalId` / `RemoteId` separately from native configuration and observations.
For UDP show requested bind and peer alongside `UdpLocalAddr` / `UdpPeerAddr`; for WS/WT show
the original URL, configured listener bind, and available native address components. For Steam
show the native target and `SteamSessionFacts`, including an absent remote identity rather than
inventing one. Label Crossbeam as a channel link without sockets. A debug alias must not be
formatted as though it still encodes an address, transport variant, or persistent player.

### Application-owned next-link failover

Netcode never inserts or changes a transport dial target. For a socket token, choose an address
from `NetcodeClient::token_server_addresses()` and explicitly construct `UdpIo::new(bind, address)`
or the appropriate stream client. A URL may use a hostname or proxy representation different
from the token's authorized address; there is no literal URL-versus-socket comparison.

For failover, the application chooses the next target and whether another attempt is appropriate.
Disconnect the old link and wait for `Disconnected` to release its routing identity, or despawn
it. Create a new entity with the next native target and a token authorizing that server, then
trigger `Connect`. Do not mutate the backend on the old entity, rename the incumbent alias,
or assume the replacement retains the same gameplay identity without an application mapping.
Netcode does not advance an internal address index that leaves the transport pointed at the
original server, and the native transport does not implement automatic next-link failover.

## Identity-bound netcode tokens

Identity tokens are an opt-in extension for servers whose authorization identity is not a socket
address. They do not change existing socket-token bytes or reduce socket-mode application data.
Choose `NetcodeConfig::identity(binding)` instead of `NetcodeConfig::socket(addresses)`.
Low-level users have equivalent `ServerConfig::identity` and `ServerConfig::socket` constructors.
The default remains an unconfigured socket server that rejects requests, not an unbound server.

For pinned WebTransport, the trusted identity is the **raw 32-byte SHA-256 certificate digest**.
The minter, server, and client must obtain the association between the intended server and that
digest through a trusted channel. An arbitrary discovery announcement does not establish trust.

```rust,ignore
use lightyear::netcode::{ConnectToken, IdentityBinding, NetcodeClient, IDENTITY_USER_DATA_BYTES};
use lightyear::netcode::{auth::Authentication, client_plugin, server_plugin::NetcodeConfig};

let binding = IdentityBinding::from_native(
    b"webtransport-cert-sha256",
    &trusted_certificate_sha256,
);
let server_config = NetcodeConfig::identity(binding)
    .with_protocol_id(protocol_id)
    .with_key(private_key);
let token = ConnectToken::build_identity(binding, protocol_id, client_id, private_key)
    .user_data([0; IDENTITY_USER_DATA_BYTES])
    .generate()?;
let token_bytes = token.try_into_bytes()?;

let client = NetcodeClient::new(
    Authentication::Token(ConnectToken::try_from_bytes(&token_bytes)?),
    client_plugin::NetcodeConfig::default(),
)?;
// Separately authenticate the same trusted certificate while dialing.
// The hexadecimal pin encodes the raw digest above.
let io = WebTransportClientIo::from_url(server_url, trusted_certificate_digest_hex);
```

The token authorizes the server; the certificate pin authenticates the native endpoint; the URL
routes the connection. None substitutes for another. `PeerId` is never a server-authentication
input. `token_server_addresses()` returns an empty slice for identity tokens; do not index it or
invent a socket address. The application still owns dialing and next-link failover.

The demonstrated identity-token pairing is pinned native WebTransport. Steam identity-token
end-to-end support is not established here; Iroh is companion-branch work, not a shipped transport
in this change. Do not infer either pairing from the transport-independent binding API.

### Canonical binding and application data

`IdentityBinding::from_native(domain, identity)` computes all 32 bytes of:

```text
SHA-256(
    b"lightyear-netcode-server-binding-v1\0"
    || u64_big_endian(domain.len())
    || domain
    || canonical_native_identity_bytes
)
```

There is no truncation, zero remapping, text normalization, or variable-prefix comparison.
For WebTransport, the domain is exactly `webtransport-cert-sha256`, and identity bytes are the
raw digest, not its hexadecimal text. For digest bytes `00 01 ... 1f`, the binding is
`01881b728d652077d53b4d2d58d2af129124d179d394bc55fe829c92457951b0`.
`IdentityBinding::from_bytes` imports an already-derived binding from an external minter.

An identity token reserves exactly 33 of the private token's 256 user-data bytes:

| Offset | Size | Meaning |
|---|---:|---|
| 0 | 1 | Identity token version, currently `1` |
| 1 | 32 | Complete server binding |
| 33 | 223 | Application data (`IDENTITY_USER_DATA_BYTES`) |

`IdentityConnectTokenBuilder::user_data` accepts exactly 223 bytes. Socket builders retain
the full 256-byte API. Server callbacks and `TokenUserData` expose the original 256-byte region;
an explicitly identity-configured application reads its data after `IDENTITY_TOKEN_HEADER_BYTES`.
Never choose the mode by inspecting an application's user-data prefix.

### Minter format and compatibility

The authenticated private address count is **zero for identity mode** and **1–32 for socket
mode**. Public token address counts must have the corresponding shape. The identity builder
emits both lists empty; the socket builder rejects empty public or explicitly supplied private
lists. Client decoding validates the public list; only the server has the key to validate the
encrypted private structure. Server validation checks configured mode first, then identity
version and all 32 binding bytes. A socket token with an identity-looking application prefix
is still a socket token and cannot enter an identity-configured server.

The envelope remains 2048 bytes and its version string remains `NETCODE 1.02\0`. **Old readers
reject identity tokens** because they reject empty address lists. Unchanged envelope size does
not imply compatibility. Existing socket bytes remain unchanged, apart from the separate policy
that ordinary client ID zero is now rejected. The application packet codec also has its own
incompatible `4c 59 01` gate; socket-token compatibility does not make old application peers compatible.

External minters must preserve Lightyear's existing **big-endian** integer encoding:

- Public envelope: version (13 bytes), protocol ID/u64, creation time/u64, expiry time/u64,
  nonce (24 bytes), encrypted private block (1024 bytes), timeout/i32, public address list,
  client-to-server key (32 bytes), server-to-client key (32 bytes), then zero padding to 2048.
- Private plaintext: client ID/u64, timeout/i32, private address list, the same two directional
  keys, user data (256 bytes), then zero padding to 1008.
- Address list: count/u32 followed by each address's family tag (`1` for IPv4, `2` for IPv6),
  IP octets (4 or 16 bytes), and port/u16. Identity lists contain only a zero count.
- Encrypt the private plaintext with XChaCha20-Poly1305 using the server's 32-byte secret key
  and the public 24-byte nonce. Associated data is the version string, protocol ID/u64 and
  expiry time/u64. Ciphertext plus its 16-byte tag forms the 1024-byte private block.

Canonical socket and identity vectors are published in
`crates/connection/netcode/src/token_vectors.json`. They were independently encrypted with PyNaCl;
Rust tests decode, decrypt, and reproduce them. The socket vector deliberately contains the same
identity-looking 256-byte user data as the identity vector. These vectors specify Lightyear's
existing codec, not byte compatibility with implementations using a different integer byte order.
Their fixed keys, nonces, and expired timestamps are **test inputs only**. Production minters must
generate fresh directional keys and nonces and enforce their application's expiry policy.

Next we will start adding systems to the client and server.
