use crate::connection::client::NetClient;
use crate::packet::packet::Packet;
use crate::prelude::{ClientId, Io};
use crate::transport::LOCAL_SOCKET;
use anyhow::Result;
use std::net::SocketAddr;

#[derive(Default)]
pub struct Client {
    id: u64,
    is_connected: bool,
}

impl Client {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            is_connected: false,
        }
    }
}

impl NetClient for Client {
    fn connect(&mut self) -> Result<()> {
        self.is_connected = true;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.is_connected
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        Ok(())
    }

    fn recv(&mut self) -> Option<Packet> {
        None
    }

    fn send(&mut self, buf: &[u8]) -> Result<()> {
        Ok(())
    }

    fn id(&self) -> ClientId {
        ClientId::Local(self.id)
    }

    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn io(&self) -> Option<&Io> {
        None
    }

    fn io_mut(&mut self) -> Option<&mut Io> {
        None
    }
}
