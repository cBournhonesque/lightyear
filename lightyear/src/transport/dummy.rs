//! Dummy io for connections that provide their own way of sending and receiving raw bytes (for example steamworks).
use crate::client::io::transport::{ClientTransportBuilder, ClientTransportEnum};
use crate::client::io::{ClientIoEventReceiver, ClientNetworkEventSender};
use crate::server::io::transport::{ServerTransportBuilder, ServerTransportEnum};
use crate::server::io::{ServerIoEventReceiver, ServerNetworkEventSender};
use crate::transport::io::IoState;
use crate::transport::{
    BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport, LOCAL_SOCKET,
};
#[cfg(not(feature = "std"))]
use alloc::boxed::Box;


use core::net::SocketAddr;
use super::error::Result;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DummyIo;

impl ClientTransportBuilder for DummyIo {
    fn connect(
        self,
    ) -> Result<(
        ClientTransportEnum,
        IoState,
        Option<ClientIoEventReceiver>,
        Option<ClientNetworkEventSender>,
    )> {
        Ok((
            ClientTransportEnum::Dummy(self),
            IoState::Connected,
            None,
            None,
        ))
    }
}

impl ServerTransportBuilder for DummyIo {
    fn start(
        self,
    ) -> Result<(
        ServerTransportEnum,
        IoState,
        Option<ServerIoEventReceiver>,
        Option<ServerNetworkEventSender>,
    )> {
        Ok((
            ServerTransportEnum::Dummy(self),
            IoState::Connected,
            None,
            None,
        ))
    }
}

impl Transport for DummyIo {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn split(self) -> (BoxedSender, BoxedReceiver) {
        (Box::new(self), Box::new(self))
    }
}

impl PacketSender for DummyIo {
    fn send(&mut self, data: &[u8], addr: &SocketAddr) -> Result<()> {
        Ok(())
    }
}

impl PacketReceiver for DummyIo {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        Ok(None)
    }
}
