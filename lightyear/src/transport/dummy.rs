//! Dummy io for connections that provide their own way of sending and receiving raw bytes (for example steamworks).
use crate::transport::{Transport, LOCAL_SOCKET};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DummyIo;

impl Transport for DummyIo {
    fn local_addr(&self) -> std::net::SocketAddr {
        LOCAL_SOCKET
    }

    fn listen(
        self,
    ) -> (
        Box<dyn crate::transport::PacketSender>,
        Box<dyn crate::transport::PacketReceiver>,
    ) {
        (Box::new(self.clone()), Box::new(self))
    }
}

impl crate::transport::PacketSender for DummyIo {
    fn send(&mut self, data: &[u8], addr: std::net::SocketAddr) -> std::io::Result<usize> {
        panic!("DummyIo::send should not be called")
    }
}

impl crate::transport::PacketReceiver for DummyIo {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], std::net::SocketAddr)>> {
        panic!("DummyIo::receive should not be called")
    }
}
