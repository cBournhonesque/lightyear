use crate::netcode::ClientId;

pub struct Events {
    // netcode
    connections: Vec<ClientId>,
    disconnections: Vec<ClientId>,
    // messages
    // replication
    // spawns: Vec<>
}
