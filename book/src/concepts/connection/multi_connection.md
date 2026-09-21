# Multiple connections and routing scope

Connections are entities, but their routes are not endpoint-local namespaces.
`NetworkingMetadata::peer_map` maps each admitted remote `PeerId` to one entity across
the entire Bevy `World`, including different endpoints and backends. The same numeric
alias cannot independently name a Steam peer and a netcode peer in that registry.
Native identities remain separate even when their projected routing aliases collide.

## Client/server scope

`PeerId::SERVER` (zero) is the server-role route, not an ordinary peer identity.
Starting a local `Server` endpoint reserves this World-wide route for that endpoint.
A conventional outgoing client connection uses it for its remote server. Server-side
child connections use it as their local identity and register their clients' nonzero
remote aliases instead.

There can be only one owner of the server route. A second independently started local
`Server` endpoint, or an outgoing remote-server connection competing with a local server
or another remote server, fails explicitly rather than replacing the owner. Stopping
or despawning a server clears the sentinel route only if that server still owns it.

Consequently, separate UDP, Steam, and WebTransport `Server` endpoints cannot simply be
started together in one `World` as independent authoritative servers. The entity model
alone does not supply a shared-listener abstraction or cross-play configuration. Use
separate Worlds for independent client/server routing scopes, or explicit P2P roles
where the intended topology is peer-to-peer. P2P is not a way to relabel an authoritative
server silently; its discovery and roster requirements still apply.

Multiple admitted remote peers with distinct aliases can coexist within the supported
topology, subject to the selected transports' and backends' actual capabilities. This
does not imply that every native transport and connection backend can be paired.

## Collision policy

All failures preserve an existing route owner. Rejected candidates expose a
`Disconnected` reason rather than notifying `Connected` observers.

| Candidate or operation | Result |
|---|---|
| Different backend on an already-owned entity, even after disconnect | Reject; replace the entity to change backend |
| Netcode token identity or another authenticated/shared remote identity already owned by another entity | Reject, including collisions across backends or endpoints; never rename |
| Explicitly requested default/debug remote alias already owned | Reject; requests are not permission to overwrite or negotiate |
| Unrequested, non-P2P default/debug remote alias collides, and is not `SERVER` | Choose a free nonzero local alias for this candidate only; preserve the native fact and incumbent |
| P2P remote claim collides, whether requested or discovered | Reject; no local renaming and no guarantee of globally consistent claimant selection |
| P2P endpoint/child self-ID requests disagree, or an admitted sender announces a different ID | Reject; no provisional public IDs or in-place renaming |
| Ordinary peer requests zero, including a netcode client, P2P peer, or host client | Reject; zero is reserved for the appropriate client/server role |
| Local `Server` start or conventional remote-server admission competes for `SERVER` | Reject the competing owner; never use the debug fallback for the sentinel |
| Explicit host-client alias collides, or its requested local and remote IDs disagree | Reject; an unrequested host alias is allocated from free nonzero local labels |
| Admission repeats on the same connected entity with the same backend and IDs | Succeeds without rebinding; changing either admitted ID is rejected |
| Disconnect, despawn, or late rejection after another entity owns a route | Release only if the departing entity still owns the route |

A repeated `LocalId` is not itself a duplicate remote route: for example, a server's
children share its local server identity. Backend-specific expectations still apply,
including netcode's token-ID check and a P2P endpoint's single self-ID.

An unrequested debug fallback guarantees only a usable local routing label. It provides
neither authentication nor agreement between Worlds; NAT and local-only transports may
produce different labels at the two ends. For authenticated client/server identity use
an appropriate explicit backend; for P2P retain the mandatory shared-roster barrier.

## Host client

`HostClient` means an in-process client and server share one `World`. It uses the host
connection path, not Crossbeam channels or a network handshake. The server must be
`Started` before the host client is admitted; a pending client stays `Connecting`.
The host path allows one host client per server and owns its backend independently
of that server's wire protocol.

The host client uses one nonzero alias for its local and remote IDs, while the server
endpoint owns `PeerId::SERVER`. Detect the role with `HostClient`, not a magic numeric ID.
Remote clients may still join that server through its configured native endpoint;
adding separate independently started server endpoints does not bypass the sentinel
restriction.

`CrossbeamIo` instead carries packets over in-memory channels and can connect separate
local apps. It follows its selected connection backend's admission policy and should
not be described as the host-client implementation.

See [connection admission](title.md#backend-selection-and-admission), the
[transport-author contract](title.md#transport-author-contract), and the tutorial's
[native dialing and failover](../../tutorial/build_client_server.md#native-dialing-and-failover).
