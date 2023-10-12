use std::net::SocketAddr;
use std::str::FromStr;

use lightyear_server::Server;
use lightyear_shared::transport::Transport;
use lightyear_shared::{Io, Protocol, UdpSocket};

use crate::protocol::CHANNEL_REGISTRY;

pub fn setup<P: Protocol>() -> anyhow::Result<Server<P>> {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let socket = UdpSocket::new(&addr)?;
    let addr = socket.local_addr();

    let io = Io::new(addr, Box::new(socket.clone()), Box::new(socket.clone()));

    // create lightyear server
    Ok(Server::new(io, 0, &CHANNEL_REGISTRY))
}
