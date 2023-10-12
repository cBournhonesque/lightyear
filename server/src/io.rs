use crossbeam_channel::Sender;

use lightyear_shared::netcode::ClientIndex;

pub struct NetcodeServerContext {
    pub connections: Sender<ClientIndex>,
    pub disconnections: Sender<ClientIndex>,
}
