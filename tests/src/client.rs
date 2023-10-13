use std::net::SocketAddr;
use std::str::FromStr;

use lightyear_shared::netcode::ConnectToken;
use lightyear_shared::transport::Transport;
use lightyear_shared::{Io, UdpSocket};

use crate::protocol::{protocol, MyProtocol};

pub fn setup(token: ConnectToken) -> anyhow::Result<lightyear_client::Client<MyProtocol>> {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let socket = UdpSocket::new(&addr)?;
    let addr = socket.local_addr();
    let io = Io::new(addr, Box::new(socket.clone()), Box::new(socket.clone()));

    // create lightyear client
    Ok(lightyear_client::Client::new(io, token, protocol()))
}
