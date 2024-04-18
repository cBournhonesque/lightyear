//! Purely local io for testing
//! Messages are sent via channels
use std::net::SocketAddr;

use bevy::utils::HashMap;
use crossbeam_channel::{Receiver, Select, Sender};
use self_cell::self_cell;
use tracing::info;

use crate::transport::io::IoState;
use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, LOCAL_SOCKET,
};

use super::error::{Error, Result};

pub struct Channels {
    sender: ChannelsSender,
    receiver: ChannelsReceiver,
}

impl Channels {
    /// Create a [`Channels`] object with a list of channels.
    /// Each channel allow us to send and receive packets to a remote client.
    pub(crate) fn new(channels: Vec<(SocketAddr, Receiver<Vec<u8>>, Sender<Vec<u8>>)>) -> Self {
        let mut remote_recv = HashMap::new();
        let mut remote_send = HashMap::new();
        for (remote_addr, recv, send) in channels {
            info!("adding remote: {:?}", remote_addr);
            remote_recv.insert(remote_addr, recv);
            remote_send.insert(remote_addr, send);
        }
        let sender = ChannelsSender { send: remote_send };
        // receiver is a self-referential struct
        let owner = ChannelsReceiverOwner { recv: remote_recv };
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
        Channels { sender, receiver }
    }
}

impl TransportBuilder for Channels {
    fn connect(self) -> Result<(TransportEnum, IoState)> {
        Ok((TransportEnum::Channels(self), IoState::Connected))
    }
}

impl Transport for Channels {
    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn split(self) -> (BoxedSender, BoxedReceiver, Option<BoxedCloseFn>) {
        (Box::new(self.sender), Box::new(self.receiver), None)
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
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
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
                            "error receiving packet from channels: {:?}",
                            e
                        ))
                        .into()),
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
    fn send(&mut self, payload: &[u8], addr: &SocketAddr) -> Result<()> {
        self.send
            .get(addr)
            .ok_or::<Error>(
                std::io::Error::other("could not find remote sender channel for address").into(),
            )?
            .try_send(payload.to_vec())
            .map_err(|_| std::io::Error::other("error sending packet to channels").into())
    }
}
