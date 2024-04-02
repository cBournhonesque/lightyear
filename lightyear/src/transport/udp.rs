//! The transport is a UDP socket
use anyhow::Context;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tracing::info;

use super::error::{Error, Result};
use super::{PacketReceiver, PacketSender, Transport};

// Maximum transmission units; maximum size in bytes of a UDP packet
// See: https://gafferongames.com/post/packet_fragmentation_and_reassembly/
const MTU: usize = 1472;

/// UDP Socket
#[derive(Clone)]
pub struct UdpSocket {
    local_addr: SocketAddr,
    /// The underlying UDP Socket. This is wrapped in an Arc<Mutex<>> so that it
    /// can be shared between threads
    socket: Option<Arc<Mutex<std::net::UdpSocket>>>,
    buffer: [u8; MTU],
}

impl UdpSocket {
    /// Create a non-blocking UDP socket
    pub fn new(local_addr: SocketAddr) -> Self {
        Self {
            local_addr,
            socket: None,
            buffer: [0; MTU],
        }
    }
}

impl Transport for UdpSocket {
    fn local_addr(&self) -> SocketAddr {
        self.socket.as_ref().map_or_else(
            || self.local_addr,
            |socket| {
                socket
                    .as_ref()
                    .lock()
                    .unwrap()
                    .local_addr()
                    .expect("error getting local addr")
            },
        )
    }

    fn connect(&mut self) -> Result<()> {
        let udp_socket = std::net::UdpSocket::bind(self.local_addr)?;
        let socket = Arc::new(Mutex::new(udp_socket));
        socket.as_ref().lock().unwrap().set_nonblocking(true)?;
        Ok(())
    }
    fn split(&mut self) -> (Box<&mut dyn PacketSender>, Box<&mut dyn PacketReceiver>) {
        // TODO: assert that the socket is connected?
        (Box::new(&mut self.clone()), Box::new(self))
    }
}

impl PacketSender for UdpSocket {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        self.socket
            .as_ref()
            .ok_or(Error::NotConnected)?
            .lock()
            .unwrap()
            .send_to(payload, address)?;
        Ok(())
    }
}

impl PacketReceiver for UdpSocket {
    /// Receives a packet from the socket, and stores the results in the provided buffer
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        match self
            .socket
            .as_ref()
            .ok_or(Error::NotConnected)?
            .lock()
            .unwrap()
            .recv_from(&mut self.buffer)
        {
            Ok((recv_len, address)) => Ok(Some((&mut self.buffer[..recv_len], address))),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Nothing to receive on the socket
                Ok(None)
            }
            // Err(e) => Err(anyhow!("error receiving packet")),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::utils::Duration;
    use std::net::SocketAddr;
    use std::str::FromStr;

    use crate::transport::udp::UdpSocket;
    use crate::transport::wrapper::conditioner::{
        ConditionedPacketReceiver, LinkConditionerConfig,
    };
    use crate::transport::{PacketReceiver, PacketSender, Transport};

    #[test]
    fn test_udp_socket() -> Result<(), anyhow::Error> {
        // let the OS assigned a port
        let local_addr = SocketAddr::from_str("127.0.0.1:0")?;

        let mut server_socket = UdpSocket::new(local_addr)?;
        let mut client_socket = UdpSocket::new(local_addr)?;

        let server_addr = server_socket.local_addr();
        let client_addr = client_socket.local_addr();

        let msg = b"hello world";
        client_socket.send(msg, &server_addr)?;

        // sleep a little to give time to the message to arrive in the socket
        std::thread::sleep(Duration::from_millis(10));

        let Some((recv_msg, address)) = server_socket.recv()? else {
            panic!("expected to receive a packet");
        };
        assert_eq!(address, client_addr);
        assert_eq!(recv_msg, msg);
        Ok(())
    }

    #[test]
    fn test_udp_socket_with_conditioner() -> Result<(), anyhow::Error> {
        use mock_instant::MockClock;

        // let the OS assigned a port
        let local_addr = SocketAddr::from_str("127.0.0.1:0")?;

        let server_socket = UdpSocket::new(local_addr)?;
        let mut client_socket = UdpSocket::new(local_addr)?;

        let server_addr = server_socket.local_addr();
        let client_addr = client_socket.local_addr();

        let mut conditioned_server_receiver = ConditionedPacketReceiver::new(
            server_socket,
            LinkConditionerConfig {
                incoming_latency: Duration::from_millis(100),
                incoming_jitter: Duration::from_millis(0),
                incoming_loss: 0.0,
            },
        );

        let msg = b"hello world";
        client_socket.send(msg, &server_addr)?;

        // TODO: why do we only this here and not in the previous test?
        // sleep a little to give time to the message to arrive in the socket
        std::thread::sleep(Duration::from_millis(10));

        // we don't receive the packet yet because the mock clock is still at 0s
        // so we add the packet to the time queue
        let None = conditioned_server_receiver.recv()? else {
            panic!("no packets should have arrived yet");
        };

        // advance a small amount, but not enough to receive the packet in the queue
        MockClock::advance(Duration::from_millis(50));
        let None = conditioned_server_receiver.recv()? else {
            panic!("no packets should have arrived yet");
        };

        MockClock::advance(Duration::from_secs(1));
        // now the packet should be available (read from the time queue)
        let Some((recv_msg, address)) = conditioned_server_receiver.recv()? else {
            panic!("expected to receive a packet");
        };
        assert_eq!(address, client_addr);
        assert_eq!(recv_msg, msg);

        Ok(())
    }
}
