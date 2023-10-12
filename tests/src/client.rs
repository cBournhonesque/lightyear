use std::net::SocketAddr;
use std::str::FromStr;

use lightyear_shared::netcode::{Client, ConnectToken};
use lightyear_shared::transport::Transport;
use lightyear_shared::{netcode, Io, Protocol, UdpSocket};

use crate::protocol::CHANNEL_REGISTRY;

pub fn setup<P: Protocol>(token: ConnectToken) -> anyhow::Result<lightyear_client::Client<P>> {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let socket = UdpSocket::new(&addr)?;
    let addr = socket.local_addr();
    let io = Io::new(addr, Box::new(socket.clone()), Box::new(socket.clone()));

    // create lightyear client
    Ok(lightyear_client::Client::new(io, token, &CHANNEL_REGISTRY))
}
