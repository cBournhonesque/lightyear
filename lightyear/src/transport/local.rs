/// Purely local io for testing
/// Messages are sent via channels
use std::net::SocketAddr;

use crossbeam_channel::{Receiver, Sender};

use super::error::{Error, Result};
use crate::transport::{PacketReceiver, PacketSender, Transport, LOCAL_SOCKET};

// TODO: this is client only; separate client/server transport traits
pub struct LocalChannel {
    sender: LocalChannelSender,
    receiver: LocalChannelReceiver,
}

impl LocalChannel {
    pub(crate) fn new(recv: Receiver<Vec<u8>>, send: Sender<Vec<u8>>) -> Self {
        let sender = LocalChannelSender { send };
        let receiver = LocalChannelReceiver {
            buffer: vec![],
            recv,
        };
        LocalChannel { receiver, sender }
    }
}

impl Transport for LocalChannel {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    fn split(&mut self) -> (&mut dyn PacketSender, &mut dyn PacketReceiver) {
        (&mut self.sender, &mut self.receiver)
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
