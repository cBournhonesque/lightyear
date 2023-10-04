use crate::clients::ClientId;
use lightyear_shared::Protocol;
use std::net::SocketAddr;

/// Connection from the server to a given client
pub struct Connection<P: Protocol> {
    pub client_address: SocketAddr,
    pub client_id: ClientId,
    pub base: lightyear_shared::Connection<P>,
}

impl<P: Protocol> Connection<P> {
    pub(crate) fn new() {}
}
