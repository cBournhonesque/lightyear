# Connection

A `Link` sends and receives bytes through a native transport. A connection adds an admitted
identity and lifecycle to that link. Neither a bare link nor default connection admission
promises authentication, a negotiated identity, or a keep-alive protocol; those guarantees
belong to the selected transport or connection backend.

Connections support conventional client/server, in-process host clients, and explicit P2P
roles. See [multiple connections and routing scope](multi_connection.md) before combining
endpoints in one Bevy `World`.

## Three identity layers

Keep these layers separate:

| Layer | Purpose | Lifetime and authority |
|---|---|---|
| Game/account identity | Persistent player, account, or application identity | Defined and authenticated by the application; not a transport address or routing alias |
| Routing identity | `PeerId`, exposed through `LocalId` and `RemoteId`, for messages and replication | A numeric alias admitted into this `World`; not a dial target or proof of identity |
| Native identity and dialing facts | Full backend identity, observed addresses, URLs, certificate digests, and discovery context | Owned by the transport; stable identity and mutable reachability are distinct facts |

`PeerId(pub u64)` is opaque. Zero (`PeerId::SERVER`) is a client/server role sentinel;
ordinary peers, including `HostClient`, use nonzero aliases. Do not infer transport,
account, or host-client role from an alias. A projection of a native identity can collide,
so retain the full native fact separately.

The tutorial specifies the [canonical numeric projections and wire representation](../../tutorial/build_client_server.md#numeric-routing-aliases).
Use those helpers rather than introducing another hash or encoding.

## Backend selection and admission

Configure one connection backend before connecting. Explicit backends require
`NoDefaultConnection` on their connection or endpoint component, which suppresses
default admission for that entity and an endpoint's children. Without an explicit
backend or opt-out, supported transports use default/debug admission.

Backend configuration is not arbitrated or frozen. Installing competing backends or
switching them on a live connection is unsupported. Host clients use an in-process
path independently of the server endpoint's wire backend.

Admission resolves the final `LocalId` and `RemoteId`, checks roles and collisions, and
registers the remote route before inserting `Connected`. Observers of `Connected` can
therefore route immediately. Rejected candidates never become `Connected`, and an admitted
connection cannot change its identity in place. Disconnect or despawn releases only the
route still owned by that entity, not a replacement's route.

### Default/debug connections

Transports supporting default admission use native facts as fallback labels when no
explicit connection backend owns the entity. No extra marker or handshake is required.
Insert `NoDefaultConnection` on the link or endpoint before starting to use only raw bytes;
on an endpoint this also disables implicit admission for its children.

Explicit debug IDs are local requests, not negotiation with the peer. NAT, browser address
visibility, and local-only transports can make the two ends choose different labels.
Only an unrequested, non-P2P default/debug remote alias may choose a free local label after
a collision. It never renames the incumbent, authenticates the peer, or establishes shared
agreement. [The collision matrix](multi_connection.md#collision-policy) describes the strict cases.

### Netcode

`NetcodeClient` / `NetcodeServer` provide authenticated connection admission using a
`ConnectToken`. A secure application service can mint tokens with the game's `protocol_id`
and `private_key`; Lightyear does not provide that secure token-delivery service.
`Authentication::Manual` is useful for testing, not a reason to distribute the server's
private key to production clients.

The token determines the client's routing identity. A preinserted `LocalId` is an
expectation to validate, not an override: manual authentication can check it before the
handshake, while an opaque token's encrypted ID is checked after authentication but before
`Connected`. Zero and duplicate client IDs are rejected.

Netcode authorization and native dialing/authentication are independent. Follow the
tutorial's [server configuration](../../tutorial/build_client_server.md#server),
[native dialing](../../tutorial/build_client_server.md#native-dialing-and-failover), and
[identity-bound token contract](../../tutorial/build_client_server.md#identity-bound-netcode-tokens).
Socket-mode token encoding is unchanged; this is not a blanket promise of upstream
netcode.io wire interoperability or support for every transport/backend pairing.

### Steam and local connections

Steam's normal `SteamPlugin` combines `SteamTransportPlugin` and `SteamConnectionPlugin`.
Use the transport half alone when composing with another explicit connection backend;
do not install both Steam connection admission and another backend on the same link.

`CrossbeamIo` is an in-memory packet transport, useful for tests and separate local apps.
It is not `HostClient`. A host client shares the server's `World`, uses the in-process host
path without a network handshake, and has a nonzero routing alias. See
[the host topology](multi_connection.md#host-client).

## P2P admission

Explicit P2P peers use ordinary nonzero aliases, not the server sentinel. A lobby endpoint
resolves one self-ID before admitting children. Requested endpoint and child IDs must agree.

UDP, WebSocket, and WebTransport lobby links use their existing bounded, own-only
`LobbyAnnounce` exchange before admission. While pending, they expose neither public IDs
nor routing-map entries; ordinary typed application messages wait. This is not a new
handshake for bare default links.

An admitted sender cannot rename itself. Duplicate claims and attempts by third-party
discovery to replace a direct binding are rejected. Collision rejection alone does not
make every peer choose the same claimant: the shared-roster agreement barrier remains
mandatory before `P2PStart` and gameplay. See the tutorial's
[P2P discovery contract](../../tutorial/build_client_server.md#p2p-discovery-admission) for
invite and native dial-context APIs.

## Transport-author contract

Use `lightyear_connection::admission` as the admission boundary:

1. Configure the backend before starting the link. Require `NoDefaultConnection` on
   its connection and endpoint components so native transport callbacks do not
   publish a default connection before authentication completes.
2. Keep full native identities, observed addresses, and dial inputs in transport-owned
   components. Resolve routing candidates without publishing provisional public IDs or
   using an alias as authentication. Finish the backend's authentication or required P2P
   discovery before admission.
3. For a transport's default/debug path, install `DefaultConnectionPlugin` and call
   `apply_default_ids(world, entity, local, remote)` with native fallback candidates. It
   respects `NoDefaultConnection` on link or endpoint, requested IDs, roles, and the
   limited local-only collision fallback. Do not bypass it to admit an opted-out link.
4. An explicit backend calls `admit(world, entity, local, remote, ())` with final IDs
   and checks its boolean result. Additional connection components can replace `()`;
   they are inserted together with `Connected`, after IDs and routing are registered.
   Central admission checks reserved roles, duplicate remote routes, and connected-ID
   changes. Never insert `Connected` directly, rewrite admitted IDs, or update
   `NetworkingMetadata::peer_map` to steal or rebind a route.
5. Use `reject(world, entity, reason)` for admission failure; it records `Disconnected`
   and triggers `Unlink`. Implement native close handling and drive normal lifecycle
   transitions on transport loss. Preserve the admitted `RemoteId` through teardown:
   the internal `Connected` removal hook (`admission::release`) and `Disconnected` hook
   release only the entity's own binding. Despawn also runs removal hooks. A stale failure
   or disconnect must never delete an incumbent or replacement route.
6. Server backends call `server::start_server(world, entity)` when ready, rather than
   inserting `Started` directly. It rejects a conflicting server route before any
   `Started` observer can run.

## Iroh companion contract

Iroh integration is companion-branch work, not shipped support in this change. Its
integration must retain the full stable endpoint identity as a native fact, separately
from mutable addresses, relay information, and other dial context. The companion contract
uses `peer_id_from_native(b"iroh", public_key_bytes)` with the complete raw 32-byte endpoint
public key. The helper specifies hashing, byte order, and zero handling; do not hash a
text rendering or dial context, or substitute the shortened alias for native identity.

The projection is only a routing candidate. Authoritative/shared identity collisions must
fail central admission, not trigger local renaming or an invented collision-negotiation
protocol. Native authentication must verify the full intended endpoint identity. Any
identity-bound netcode composition must also follow the tutorial's separate authorization
contract; no Iroh identity-token end-to-end support is claimed here.
