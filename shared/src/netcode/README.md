# Description

This module is a Rust implementation of the [netcode.io](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md) standard.
The code is almost entirely copy-pasted from Renet.

This standard determines how a client can securely connect to a dedicated game server and start exchanging data through UDP.

A separate web server is used to handle the initial connection (via HTTPS) by returning a secure <i>connect_token</i>.

Note that how the client talks to web server to get the connect_token is not part of the standard.
The standard only explains how to use the connect_token to connect to the game server via UDP.


# Other rust implementations

- https://github.com/jaynus/netcode.io/blob/master/rust/src/client.rs
- Renet
- Naia

