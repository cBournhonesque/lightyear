use std::net::SocketAddr;
use std::str::FromStr;

use lightyear_server::Server;
use lightyear_shared::transport::Transport;
use lightyear_shared::{Io, UdpSocket};

use crate::protocol::{protocol, MyProtocol};

pub fn setup() -> anyhow::Result<Server<MyProtocol>> {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let socket = UdpSocket::new(&addr)?;
    let addr = socket.local_addr();

    let io = Io::new(addr, Box::new(socket.clone()), Box::new(socket.clone()));

    // create lightyear server
    Ok(Server::new(io, 0, protocol()))
}
