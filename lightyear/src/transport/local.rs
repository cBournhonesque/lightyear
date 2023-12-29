/// Purely local io for testing
/// Messages are sent via channels
use std::net::SocketAddr;

use crossbeam_channel::{Receiver, Sender};

use crate::transport::{PacketReceiver, PacketSender, Transport, LOCAL_SOCKET};

// TODO: this is client only; separate client/server transport traits
#[derive(Clone)]
pub struct LocalChannel {
    recv: Receiver<Vec<u8>>,
    send: Sender<Vec<u8>>,
}

impl LocalChannel {
    pub(crate) fn new(recv: Receiver<Vec<u8>>, send: Sender<Vec<u8>>) -> Self {
        LocalChannel { recv, send }
    }
}

impl Transport for LocalChannel {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn listen(self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
        let sender = LocalChannelSender { send: self.send };
        let receiver = LocalChannelReceiver {
            buffer: vec![],
            recv: self.recv,
        };
        (Box::new(sender), Box::new(receiver))
    }
}

struct LocalChannelReceiver {
    buffer: Vec<u8>,
    recv: Receiver<Vec<u8>>,
}

impl PacketReceiver for LocalChannelReceiver {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        self.recv.try_recv().map_or_else(
            |e| match e {
                crossbeam_channel::TryRecvError::Empty => Ok(None),
                _ => Err(std::io::Error::other(format!(
                    "error receiving packet: {:?}",
                    e
                ))),
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
    fn send(&mut self, payload: &[u8], _: &SocketAddr) -> std::io::Result<()> {
        self.send
            .try_send(payload.to_vec())
            .map_err(|_| std::io::Error::other("error sending packet"))
    }
}
