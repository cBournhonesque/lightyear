# Netcode


## Introduction

I use the term 'netcode' to refer to code that creates an abstraction of a connection between two peers.

Our transport layer only allows us to send/receive raw packets to a remote address.
We want to be able to create a 'connection', which means:
- sending some handshake packets
- sending some keep-alive packets to check that the confirmation is still open
- performing authentication


I call this 'netcode' because I am using the [netcode.io](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md) standard created
by Glenn Fiedler (of GafferOnGames fame).

For my purpose I am using [this](https://github.com/benny-n/netcode) Rust implementation of the standard.

The standard provides:
- authentication
- encryption
- etc.
