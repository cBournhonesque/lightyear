use crossbeam_channel::Sender;

use lightyear_shared::netcode::ClientId;

pub struct NetcodeServerContext {
    pub connections: Sender<ClientId>,
    pub disconnections: Sender<ClientId>,
}
