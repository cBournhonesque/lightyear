use crossbeam_channel::Sender;

use crate::netcode::ClientId;

pub struct NetcodeServerContext {
    pub connections: Sender<ClientId>,
    pub disconnections: Sender<ClientId>,
}
