//! Dummy io for connections that provide their own way of sending and receiving raw bytes (for example steamworks).
use std::net::SocketAddr;

use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, LOCAL_SOCKET,
};

use super::error::Result;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DummyIo;

impl TransportBuilder for DummyIo {
    fn connect(self) -> Result<TransportEnum> {
        Ok(TransportEnum::Dummy(self))
    }
}

impl Transport for DummyIo {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn split(self) -> (BoxedSender, BoxedReceiver, Option<BoxedCloseFn>) {
        (Box::new(self), Box::new(self), None)
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
