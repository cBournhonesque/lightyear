//! Dummy io for connections that provide their own way of sending and receiving raw bytes (for example steamworks).
use super::error::Result;
use crate::transport::{PacketReceiver, PacketSender, Transport, LOCAL_SOCKET};
use std::net::SocketAddr;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DummyIo;

impl Transport for DummyIo {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    fn split(&mut self) -> (&mut (dyn PacketSender + '_), &mut (dyn PacketReceiver + '_)) {
        panic!("DummyIo::split should not be called")
    }
}

impl PacketSender for DummyIo {
    fn send(&mut self, data: &[u8], addr: &SocketAddr) -> Result<()> {
        panic!("DummyIo::send should not be called")
    }
}

impl PacketReceiver for DummyIo {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        panic!("DummyIo::receive should not be called")
    }
}
