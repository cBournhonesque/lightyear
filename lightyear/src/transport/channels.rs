use bevy::utils::HashMap;
/// Purely local io for testing
/// Messages are sent via channels
use std::net::SocketAddr;

use crossbeam_channel::{Receiver, Select, Sender};
use self_cell::self_cell;
use tracing::info;

use crate::transport::{PacketReceiver, PacketSender, Transport, LOCAL_SOCKET};

#[derive(Clone)]
pub struct Channels {
    // sender channels from remotes
    remote_recv: HashMap<SocketAddr, Receiver<Vec<u8>>>,
    remote_send: HashMap<SocketAddr, Sender<Vec<u8>>>,
}

impl Channels {
    pub(crate) fn new() -> Self {
        Channels {
            remote_recv: HashMap::new(),
            remote_send: HashMap::new(),
        }
    }

    /// Add a new remote service that we can send packets to
    /// - it should provide a Sender (the remote will have the corresponding Receiver)
    /// - it should provide a Receiver (the remote will have the corresponding Sender)
    pub(crate) fn add_new_remote(
        &mut self,
        remote_addr: SocketAddr,
        remote_recv: Receiver<Vec<u8>>,
        remote_send: Sender<Vec<u8>>,
    ) {
        self.remote_recv.insert(remote_addr, remote_recv);
        self.remote_send.insert(remote_addr, remote_send);
    }
}

impl Transport for Channels {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn listen(self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
        let sender = ChannelsSender {
            send: self.remote_send,
        };

        // receiver is a self-referential struct
        let owner = ChannelsReceiverOwner {
            recv: self.remote_recv,
        };
        let receiver = ChannelsReceiver::new(owner, |o| {
            let mut id_map = HashMap::new();
            let mut select = Select::new();
            for (addr, recv) in o.recv.iter() {
                let idx = select.recv(recv);
                id_map.insert(idx, *addr);
            }
            ChannelsReceiverDependent {
                buffer: vec![],
                select,
                id_map,
            }
        });

        (Box::new(sender), Box::new(receiver))
    }
}

struct ChannelsReceiverOwner {
    recv: HashMap<SocketAddr, Receiver<Vec<u8>>>,
}
struct ChannelsReceiverDependent<'a> {
    buffer: Vec<u8>,
    select: Select<'a>,
    id_map: HashMap<usize, SocketAddr>,
}
self_cell!(
    struct ChannelsReceiver {
        owner: ChannelsReceiverOwner,

        #[covariant]
        dependent: ChannelsReceiverDependent,
    }
);

impl PacketReceiver for ChannelsReceiver {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        self.with_dependent_mut(|owner, dependent| {
            let op = dependent.select.try_select().map_or_else(
                |e| Ok(None),
                |op| {
                    let addr = dependent.id_map.get(&op.index()).unwrap();
                    let recv = owner.recv.get(addr).unwrap();
                    match op.recv(recv) {
                        Ok(data) => {
                            dependent.buffer = data;
                            Ok(Some((dependent.buffer.as_mut_slice(), *addr)))
                        }
                        Err(e) => Err(std::io::Error::other(format!(
                            "error receiving packet: {:?}",
                            e
                        ))),
                    }
                },
            );
            op
        })
    }
}

struct ChannelsSender {
    send: HashMap<SocketAddr, Sender<Vec<u8>>>,
}

impl PacketSender for ChannelsSender {
    fn send(&mut self, payload: &[u8], addr: &SocketAddr) -> std::io::Result<()> {
        self.send
            .get(addr)
            .ok_or(std::io::Error::other(
                "could not find remote sender channel for address",
            ))
            .unwrap()
            .try_send(payload.to_vec())
            .map_err(|_| std::io::Error::other("error sending packet"))
    }
}
