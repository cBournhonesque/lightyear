/*! Netcode.io protocol to establish a connection on top of an unreliable transport

# netcode

 The `netcode` crate implements the [netcode](https://github.com/networkprotocol/netcode)
 network protocol created by [Glenn Fiedler](https://gafferongames.com).

 `netcode` is a UDP-based protocol that provides secure, connection-based data transfer.

 Since the protocol is meant to be used to implement multiplayer games, its API is designed
 to be used in a game loop, where the server and client are updated at a fixed rate (e.g., 60Hz).

 ## Protocol

 The three main components of the netcode protocol are:
 * Dedicated [`Servers`](Server).
 * [`Clients`](NetcodeClient).
 * The web backend - a service that authenticates clients and generates [`ConnectTokens`](ConnectToken).

 The protocol does not specify how the web backend should be implemented, but it should probably be a typical HTTPS server
 that provides a means for clients to authenticate and request connection tokens.

 The sequence of operations for a client to connect to a server is as follows:

 1. The `Client` authenticates with the web backend service. (e.g., by OAuth or some other means)
 2. The authenticated `Client` requests a connection token from the web backend.
 3. The web backend generates a [`ConnectToken`] and sends it to the `Client`. (e.g., as a JSON response)
 4. The `Client` uses the token to connect to a dedicated `Server`.
 5. The `Server` makes sure the token is valid and allows the `Client` to connect.
 6. The `Client` and `Server` can now exchange encrypted and signed UDP packets.

 To learn more about the netcode protocol, see the upstream [specification](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md).

 ## Server

 The netcode server is responsible for managing the state of the clients and sending/receiving packets.

 The server should run as a part of the game loop, process incoming packets and send updates to the clients.

 To create a server:
  * Provide the address you intend to bind to.
  * Provide the protocol id - a `u64` that uniquely identifies your app.
  * Provide a private key - a `u8` array of length 32. If you don't have one, you can generate one with `netcode::generate_key()`.
  * Optionally provide a [`ServerConfig`] - a struct that allows you to customize the server's behavior.

 ```
use std::{thread, time::{Instant, Duration}, net::SocketAddr};
use crate::lightyear::connection::netcode::{generate_key, NetcodeServer, MAX_PACKET_SIZE};
use lightyear::prelude::server::*;
use crate::lightyear::transport::io::BaseIo;

// Create an io
let mut io = IoConfig::from_transport(ServerTransport::Dummy).start().unwrap();

// Create a server
let protocol_id = 0x11223344;
let private_key = generate_key(); // you can also provide your own key
let mut server = NetcodeServer::new(protocol_id, private_key).unwrap();

// Run the server at 60Hz
let start = Instant::now();
let tick_rate = Duration::from_secs_f64(1.0 / 60.0);
loop {
    let elapsed = start.elapsed().as_secs_f64();
    server.update(elapsed, &mut io);
    while let Some((packet, from)) = server.recv() {
       // ...
    }
    # break;
    thread::sleep(tick_rate);
}
```

 ## Client

 The netcode client connects to the server and communicates using the same protocol.

 Like the server, the game client should run in a loop to process incoming data,
 send updates to the server, and maintain a stable connection.

 To create a client:
  * Provide a **connect token** - a `u8` array of length 2048 serialized from a [`ConnectToken`].
  * Optionally provide a [`ClientConfig`] - a struct that allows you to customize the client's behavior.

 ```
use std::{thread, time::{Instant, Duration}, net::SocketAddr};
use lightyear::prelude::client::*;
use crate::lightyear::connection::netcode::{generate_key, ConnectToken, NetcodeClient, MAX_PACKET_SIZE};

// Create an io
let mut io = IoConfig::from_transport(ClientTransport::Dummy).connect().unwrap();

// Generate a connection token for the client
let protocol_id = 0x11223344;
let private_key = generate_key(); // you can also provide your own key
let client_id = 123u64; // globally unique identifier for an authenticated client
let server_address = "127.0.0.1:12345"; // the server's public address (can also be multiple addresses)
let connect_token = ConnectToken::build("127.0.0.1:12345", protocol_id, client_id, private_key)
    .generate()
    .unwrap();

// Start the client
let token_bytes = connect_token.try_into_bytes().unwrap();
let mut client = NetcodeClient::new(&token_bytes).unwrap();
client.connect();

// Run the client at 60Hz
let start = Instant::now();
let tick_rate = Duration::from_secs_f64(1.0 / 60.0);
loop {
    let elapsed = start.elapsed().as_secs_f64();
    client.try_update(elapsed, &mut io).ok();
    if let Some(packet) = client.recv() {
        // ...
    }
    # break;
    thread::sleep(tick_rate);
}
```
*/

pub use client::{connection::Client, ClientConfig, ClientState, NetcodeClient};
pub use crypto::{generate_key, try_generate_key, Key};
pub use error::{Error, Result};
pub use server::{connection::Server, Callback, ClientId, NetcodeServer, ServerConfig};
pub use token::{ConnectToken, ConnectTokenBuilder, InvalidTokenError};

mod bytes;
mod client;
mod crypto;
pub(crate) mod error;
mod packet;
mod replay;
mod server;
mod token;
mod utils;

pub(crate) const MAC_BYTES: usize = 16;
pub(crate) const MAX_PKT_BUF_SIZE: usize = 1300;
pub(crate) const CONNECTION_TIMEOUT_SEC: i32 = 15;
pub(crate) const PACKET_SEND_RATE_SEC: f64 = 1.0 / 10.0;

/// The size of a private key in bytes.
pub const PRIVATE_KEY_BYTES: usize = 32;
/// The size of the user data in a connect token in bytes.
pub const USER_DATA_BYTES: usize = 256;
/// The size of the connect token in bytes.
pub const CONNECT_TOKEN_BYTES: usize = 2048;
/// The maximum size of a packet in bytes.
pub const MAX_PACKET_SIZE: usize = 1200;
/// The version of the netcode protocol implemented by this crate.
pub const NETCODE_VERSION: &[u8; 13] = b"NETCODE 1.02\0";
