use crate::_reexport::ReadWordBuffer;
use crate::connection::client::NetClient;
use crate::prelude::{ClientId, Io};
use std::collections::VecDeque;
use std::net::SocketAddr;
use steamworks::ClientManager;

pub struct Client {
    client: steamworks::Client<ClientManager>,
    packet_queue: VecDeque<ReadWordBuffer>,
}

impl Client {
    pub fn new(client: steamworks::Client<ClientManager>) -> Result<Self, InvalidHandle> {
        Self {
            client,
            packet_queue: VecDeque::new(),
        }
    }
}

impl NetClient for Client {
    fn connect(&mut self) -> anyhow::Result<()> {
        todo!()
    }

    fn is_connected(&self) -> bool {
        todo!()
    }

    fn try_update(&mut self, delta_ms: f64) -> anyhow::Result<()> {
        todo!()
    }

    fn recv(&mut self) -> Option<ReadWordBuffer> {
        todo!()
    }

    fn send(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        todo!()
    }

    fn id(&self) -> ClientId {
        todo!()
    }

    fn local_addr(&self) -> SocketAddr {
        todo!()
    }

    fn io(&self) -> Option<&Io> {
        todo!()
    }

    fn io_mut(&mut self) -> Option<&mut Io> {
        todo!()
    }
}
