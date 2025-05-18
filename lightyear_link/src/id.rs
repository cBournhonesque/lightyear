use core::net::SocketAddr;

#[derive(Debug, PartialEq, Clone)]
pub enum LinkId {
    Channel,
    Udp(SocketAddr),
}
