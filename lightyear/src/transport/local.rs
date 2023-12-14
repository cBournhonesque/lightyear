//! Purely local io for testing: messages are sent via channels
use std::net::SocketAddr;

use crossbeam_channel::{Receiver, Sender};

use crate::transport::{PacketReceiver, PacketSender, Transport};

pub(crate) const LOCAL_SOCKET: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
    0,
);

#[derive(Clone)]
pub struct LocalChannel {
    recv: Receiver<Vec<u8>>,
    send: Sender<Vec<u8>>,
    buffer: Vec<u8>,
}

impl LocalChannel {
    pub(crate) fn new() -> Self {
        let (send1, recv1) = crossbeam_channel::unbounded();
        LocalChannel {
            recv: recv1,
            send: send1,
            buffer: vec![],
        }
    }
}

impl Transport for LocalChannel {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn listen(&mut self) -> anyhow::Result<(Box<dyn PacketSender>, Box<dyn PacketReceiver>)> {
        let (send, recv) = crossbeam_channel::unbounded();
        let sender = LocalChannelSender { send };
        let receiver = LocalChannelReceiver {
            buffer: vec![],
            recv,
        };
        Ok((Box::new(sender), Box::new(receiver)))
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
