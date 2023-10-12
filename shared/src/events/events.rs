use crate::netcode::{ClientId, ClientIndex};
use crate::Protocol;

pub struct Events<P: Protocol> {
    // netcode
    connections: Vec<ClientId>,
    disconnections: Vec<ClientId>,
    // messages
    // replication
    // spawns: Vec<>
}
