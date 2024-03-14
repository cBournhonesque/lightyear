# Connection

## Introduction

Our transport layer only allows us to send/receive raw packets to a remote address.
But we want to be able to create a stateful 'connection' where we know that two peers are connected.

To establish that connection, that needs to be some machinery that runs on top of the transport layer and takes care of:
- sending handshake packets to authenticate the connection
- sending keep-alive packets to check that the connection is still open
- storing the list of connected remote peers
- etc.

In lightyear, it is possible to use different connection types, via the two traits
`NetClient` and `NetServer`.

Multiple implementations are provided:
- Netcode
- Steam


## Netcode

This implementation is based on the [netcode.io](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md) standard created
by Glenn Fiedler (of GafferOnGames fame). It describes a protocol to establish a secure connection between two peers, provided
that there is a transport layer to exchange packets.

For my purpose I am using [this](https://github.com/benny-n/netcode) Rust implementation of the standard.

You can use the Netcode connection by using the `NetcodeClient` and `NetcodeServer` structs, coupled with any of the available
transports (Udp, WebTransport, etc.)

## Steam

This implementation is based on the Steamworks SDK. 
