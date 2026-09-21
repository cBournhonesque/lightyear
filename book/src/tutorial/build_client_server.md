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

Choose the connection backend before connecting. A connection entity keeps that backend even after
disconnecting; spawn a new entity to use another backend. Conflicting backend markers fail with a
`Disconnected` reason rather than silently selecting one.

Remote peer IDs are unique across all endpoints in the same Bevy `World`. A backend registers the
resolved identity before adding `Connected`, so connection observers can immediately route to it.
Authenticated, P2P, and explicitly requested duplicate IDs are rejected without notifying
`Connected` observers. An unrequested default/debug ID may select a free local alias after a
collision; it never renames the incumbent or establishes cross-peer agreement. Normal disconnect
or despawn releases the binding.

For netcode, a preinserted `LocalId` is an expectation, not an override of the token. Manual
authentication can validate it before the handshake. Opaque tokens keep that ID encrypted, so their
expected ID is checked after authentication but before `Connected`.

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
        NetcodeServer::new(NetcodeConfig::default().with_expected_addresses([SERVER_ADDR])),
        UdpEndpoint::new(SERVER_ADDR),
        Server,
    ))
    .id();
commands.trigger(Start { entity: server });
```

`NetcodeServer` selects the authenticated connection backend and adds the `Server` role.
Its `expected_addresses` are the server addresses authorized by incoming tokens, not a transport
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

WebSocket clients use `WebSocketClientIo::from_url(config, "wss://game.example/session")` or
`from_host(config, WebSocketScheme::Secure, "game.example", 443)`. WebTransport clients use
`WebTransportClientIo::from_url("https://game.example/session", certificate_digest)` or
`from_host("game.example", 443, certificate_digest)`. Hostnames stay intact for DNS, TLS and browser
APIs; do not pre-resolve them into IP addresses. The host constructors also handle unbracketed IPv6.

WebSocket's `ServerConfig` owns its bind configuration. `WebTransportEndpoint` owns `bind_addr`
and `certificate`. Read `WebSocketLocalAddr` / `WebTransportLocalAddr` after binding for the actual
listener address; their corresponding `PeerAddr` types describe observed native sessions, not
dial inputs. Browsers do not expose socket addresses, so those observations may be absent.

Netcode never inserts or changes a transport dial target. For a socket token, choose an address
from `NetcodeClient::token_server_addresses()` and explicitly construct `UdpIo::new(bind, address)`
or the appropriate stream client. A URL may use a hostname or proxy representation different
from the token's authorized address; there is no literal URL-versus-socket comparison.

For failover, disconnect the old link and wait for `Disconnected` to release its routing identity.
Create a new entity with the next native target and a token authorizing that server, then trigger
`Connect`. Netcode does not advance an internal address index that leaves the transport pointed
at the original server.

Next we will start adding systems to the client and server.
