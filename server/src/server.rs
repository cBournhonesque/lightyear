use crate::clients::ClientId;
use lightyear_shared::{Connection, Protocol};
use std::collections::HashMap;

pub struct Server<P: Protocol> {
    // Config

    // Clients
    connections: HashMap<ClientId, Connection<P>>,
}
