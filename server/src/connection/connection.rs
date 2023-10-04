use crate::clients::ClientId;
use lightyear_shared::Protocol;
use std::net::SocketAddr;

/// Connection from the server to a given client
pub struct Connection<P: Protocol> {
    pub client_address: SocketAddr,
    pub client_id: ClientId,
    pub base: lightyear_shared::Connection<P>,
}

// WE WANT TO COMPLETELY SEPARATE TWO THINGS, LIKE RENET:
// - the IO/TRANSPORT, which actually sends the buffered messages, receives from io and buffers (netcode.io + transport) (LOOP INTERNALLY)
// - the CHANNEL/RELIABILITY, which buffers the messages into buffers, and reads from the buffers into actual channels (CALLED BY USER)
impl<P: Protocol> Connection<P> {
    pub(crate) fn new() {}
}
