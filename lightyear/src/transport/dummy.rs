//! Dummy io for connections that provide their own way of sending and receiving raw bytes (for example steamworks).
use std::net::SocketAddr;

use super::error::Result;
use crate::{
    client::io::{
        transport::{ClientTransportBuilder, ClientTransportEnum},
        ClientIoEventReceiver, ClientNetworkEventSender,
    },
    server::io::{
        transport::{ServerTransportBuilder, ServerTransportEnum},
        ServerIoEventReceiver, ServerNetworkEventSender,
    },
    transport::{
        io::IoState, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
        LOCAL_SOCKET,
    },
};

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
        panic!("DummyIo::send should not be called")
    }
}

impl PacketReceiver for DummyIo {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        panic!("DummyIo::receive should not be called")
    }
}
