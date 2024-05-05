# Connection

## Introduction

Our transport layer only allows us to send/receive raw packets to a remote address.
But we want to be able to create a stateful 'connection' where we know that two peers are connected.

To establish that connection, that needs to be some machinery that runs on top of the transport layer and takes care of:
- sending handshake packets to authenticate the connection
- sending keep-alive packets to check that the connection is still open
- storing the list of connected remote peers
- etc.

`lightyear` uses the traits `NetClient` and `NetServer` to abstract over the connection logic.

Multiple implementations are provided:
- Netcode
- Steam
- Local


## Netcode

This implementation is based on the [netcode.io](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md) standard created
by Glenn Fiedler (of GafferOnGames fame). It describes a protocol to establish a secure connection between two peers, provided
that there is an unoredered unreliable transport layer to exchange packets.

For my purpose I am using [this](https://github.com/benny-n/netcode) Rust implementation of the standard.

You can use the Netcode connection by using the `NetcodeClient` and `NetcodeServer` structs, coupled with any of the available
transports (Udp, WebTransport, etc.)

To connect to a game server, the client needs to send a `ConnectToken` to the game server to start the connection process.

There are several ways to obtain a `ConnectToken`:
- the client can request a `ConnectToken` via a secure (e.g. HTTPS) connection from a backend server.
The server must use the same `protocol_id` and `private_key` as the game servers.
The backend server could be a dedicated webserver; or the game server itself, if it has a way to
establish secure connection.
- when testing, it can be convenient for the client to create its own `ConnectToken` manually.
You can use `Authentication::Manual` for those cases.

Currently `lightyear` does not provide any functionality to let a game server send a `ConnectToken` securely to a client.
You will have to handle this logic youself.


## Steam

This implementation is based on the Steamworks SDK. 

## Local

Local connections are used when running in host-server mode: the server and the client are running in the same bevy App.
No packets are actually sent over the network since the client and server share the same `World`.
