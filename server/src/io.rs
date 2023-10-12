use std::io::Result;
use std::net::SocketAddr;

use crossbeam_channel::Sender;

use lightyear_shared::netcode::ClientIndex;
use lightyear_shared::transport::PacketSender;
use lightyear_shared::Io;

pub struct NetcodeServerContext {
    pub connections: Sender<ClientIndex>,
    pub disconnections: Sender<ClientIndex>,
}
