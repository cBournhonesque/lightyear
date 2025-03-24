use crate::client::io::Io;
use crate::connection::client::{ConnectionError, ConnectionState, NetClient};
use crate::packet::packet_builder::RecvPayload;
use crate::prelude::ClientId;
use crate::transport::LOCAL_SOCKET;
use core::net::SocketAddr;

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
    fn connect(&mut self) -> Result<(), ConnectionError> {
        self.is_connected = true;
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.is_connected = false;
        Ok(())
    }

    fn state(&self) -> ConnectionState {
        if self.is_connected {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected { reason: None }
        }
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<(), ConnectionError> {
        Ok(())
    }

    fn recv(&mut self) -> Option<RecvPayload> {
        None
    }

    fn send(&mut self, buf: &[u8]) -> Result<(), ConnectionError> {
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
