/// Purely local io for testing
/// Messages are sent via channels
use std::net::SocketAddr;

use crossbeam_channel::{Receiver, Sender};

use crate::transport::io::{IoEventReceiver, IoState};
use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, LOCAL_SOCKET,
};

use super::error::{Error, Result};

// TODO: this is client only; separate client/server transport traits
pub(crate) struct LocalChannelBuilder {
    pub(crate) recv: Receiver<Vec<u8>>,
    pub(crate) send: Sender<Vec<u8>>,
}

impl TransportBuilder for LocalChannelBuilder {
    fn connect(self) -> Result<(TransportEnum, IoState, Option<IoEventReceiver>)> {
        Ok((
            TransportEnum::LocalChannel(LocalChannel {
                sender: LocalChannelSender { send: self.send },
                receiver: LocalChannelReceiver {
                    buffer: vec![],
                    recv: self.recv,
                },
            }),
            IoState::Connected,
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

    fn split(self) -> (BoxedSender, BoxedReceiver, Option<BoxedCloseFn>) {
        (Box::new(self.sender), Box::new(self.receiver), None)
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
                _ => Err(Error::Io(std::io::Error::other(format!(
                    "error receiving packet: {:?}",
                    e
                )))),
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
            .map_err(|e| std::io::Error::other("error sending packet").into())
    }
}
