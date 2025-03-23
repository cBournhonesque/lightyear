/// Purely local io for testing
/// Messages are sent via channels
#[cfg(not(feature = "std"))]
use {
    alloc::{boxed::Box, format, vec, vec::Vec},
    no_std_io2::io,
};
#[cfg(feature = "std")]
use {
    std::io,
};
use core::net::SocketAddr;


use crossbeam_channel::{Receiver, Sender};

use crate::client::io::transport::{ClientTransportBuilder, ClientTransportEnum};
use crate::client::io::{ClientIoEventReceiver, ClientNetworkEventSender};
use crate::transport::io::IoState;
use crate::transport::{
    BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport, LOCAL_SOCKET,
};

use super::error::{Error, Result};

// TODO: this is client only; separate client/server transport traits
pub(crate) struct LocalChannelBuilder {
    pub(crate) recv: Receiver<Vec<u8>>,
    pub(crate) send: Sender<Vec<u8>>,
}

impl LocalChannelBuilder {
    fn build(self) -> LocalChannel {
        LocalChannel {
            sender: LocalChannelSender { send: self.send },
            receiver: LocalChannelReceiver {
                buffer: vec![],
                recv: self.recv,
            },
        }
    }
}

impl ClientTransportBuilder for LocalChannelBuilder {
    fn connect(
        self,
    ) -> Result<(
        ClientTransportEnum,
        IoState,
        Option<ClientIoEventReceiver>,
        Option<ClientNetworkEventSender>,
    )> {
        Ok((
            ClientTransportEnum::LocalChannel(self.build()),
            IoState::Connected,
            None,
            None,
        ))
    }
}

pub struct LocalChannel {
    sender: LocalChannelSender,
    receiver: LocalChannelReceiver,
}

impl Transport for LocalChannel {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn split(self) -> (BoxedSender, BoxedReceiver) {
        (Box::new(self.sender), Box::new(self.receiver))
    }
}

struct LocalChannelReceiver {
    buffer: Vec<u8>,
    recv: Receiver<Vec<u8>>,
}

impl PacketReceiver for LocalChannelReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        self.recv.try_recv().map_or_else(
            |e| match e {
                crossbeam_channel::TryRecvError::Empty => Ok(None),
                _ => Err(Error::Io(io::Error::other(format!(
                    "error receiving packet: {:?}",
                    e
                ).as_str()))),
            },
            |data| {
                self.buffer = data;
                Ok(Some((self.buffer.as_mut_slice(), LOCAL_SOCKET)))
            },
        )
    }
}

struct LocalChannelSender {
    send: Sender<Vec<u8>>,
}

impl PacketSender for LocalChannelSender {
    fn send(&mut self, payload: &[u8], _: &SocketAddr) -> Result<()> {
        self.send
            .try_send(payload.to_vec())
            .map_err(|e| io::Error::other("error sending packet").into())
    }
}
