/// Purely local io for testing
/// Messages are sent via channels
use std::net::SocketAddr;

use crossbeam_channel::{Receiver, Sender};

use crate::transport::{PacketReceiver, PacketSender, Transport};

const LOCAL_SOCKET: SocketAddr = SocketAddr::new(
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
}

impl PacketReceiver for LocalChannel {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        self.recv.try_recv().map_or_else(
            |e| match e {
                crossbeam_channel::TryRecvError::Empty => Ok(None),
                _ => Err(std::io::Error::other(format!(
                    "error receiving packet: {:?}",
                    e
                ))),
            },
            |mut data| {
                self.buffer = data;
                Ok(Some((self.buffer.as_mut_slice(), LOCAL_SOCKET)))
            },
        )
    }
}

impl PacketSender for LocalChannel {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> std::io::Result<()> {
        self.send
            .try_send(payload.to_vec())
            .map_err(|e| std::io::Error::other("error sending packet"))
    }
}
