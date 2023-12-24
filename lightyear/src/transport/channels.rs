// use bevy::utils::HashMap;
// /// Purely local io for testing
// /// Messages are sent via channels
// use std::net::SocketAddr;
//
// use crossbeam_channel::{Receiver, Sender};
//
// use crate::transport::{PacketReceiver, PacketSender, Transport};
//
// #[derive(Clone)]
// pub struct Channels {
//     addr: SocketAddr,
//     // local receiver channel
//     recv: Receiver<Vec<u8>>,
//     // local sender channel
//     send: Option<Sender<Vec<u8>>>,
//     // sender channels from remotes
//     remote_recv: HashMap<SocketAddr, Receiver<Vec<u8>>>,
//     remote_send: HashMap<SocketAddr, Sender<Vec<u8>>>,
//     buffer: Vec<u8>,
// }
//
// impl Channels {
//     pub(crate) fn new(addr: SocketAddr) -> Self {
//         let (send, recv) = crossbeam_channel::unbounded();
//         Channels {
//             addr,
//             recv,
//             send: Some(send),
//             remote_recv: HashMap::new(),
//             remote_send: HashMap::new(),
//             buffer: vec![],
//         }
//     }
//
//     pub(crate) fn add_new_remote(&mut self, remote_addr: SocketAddr, remote_send: Sender<Vec<u8>>) {
//         self.remote_send.insert(remote_addr, remote_send);
//     }
// }
//
// impl Transport for Channels {
//     fn local_addr(&self) -> SocketAddr {
//         self.addr
//     }
//
//     fn listen(&mut self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
//         let sender = LocalChannelSender {
//             send: self.send.clone(),
//         };
//         let receiver = ChannelsReceiver {
//             buffer: vec![],
//             recv: self.recv.clone(),
//         };
//         (Box::new(sender), Box::new(receiver))
//     }
// }
//
// struct ChannelsReceiver {
//     buffer: Vec<u8>,
//     recv: Receiver<Vec<u8>>,
// }
//
// // TODO: would need an async runtime polling through each of the remote_recv channels
// impl PacketReceiver for ChannelsReceiver {
//     fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
//         self.recv.try_recv().map_or_else(
//             |e| match e {
//                 crossbeam_channel::TryRecvError::Empty => Ok(None),
//                 _ => Err(std::io::Error::other(format!(
//                     "error receiving packet: {:?}",
//                     e
//                 ))),
//             },
//             |data| {
//                 self.buffer = data;
//                 Ok(Some((self.buffer.as_mut_slice(), LOCAL_SOCKET)))
//             },
//         )
//     }
// }
//
// struct LocalChannelSender {
//     send: Sender<Vec<u8>>,
// }
//
// impl PacketSender for LocalChannelSender {
//     fn send(&mut self, payload: &[u8], _: &SocketAddr) -> std::io::Result<()> {
//         self.send
//             .try_send(payload.to_vec())
//             .map_err(|_| std::io::Error::other("error sending packet"))
//     }
// }
