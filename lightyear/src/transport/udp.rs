//! The transport is a UDP socket
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Context;

use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, MTU,
};

use super::error::Result;

pub struct UdpSocketBuilder {
    pub(crate) local_addr: SocketAddr,
}

impl TransportBuilder for UdpSocketBuilder {
    async fn connect(self) -> Result<TransportEnum> {
        let udp_socket = std::net::UdpSocket::bind(self.local_addr)?;
        let local_addr = udp_socket.local_addr()?;
        let socket = Arc::new(Mutex::new(udp_socket));
        socket.as_ref().lock().unwrap().set_nonblocking(true)?;
        let sender = UdpSocketBuffer {
            socket: socket.clone(),
            buffer: [0; MTU],
        };
        let receiver = sender.clone();
        Ok(TransportEnum::UdpSocket(UdpSocket {
            local_addr,
            sender,
            receiver,
        }))
    }
}

/// UDP Socket
pub struct UdpSocket {
    local_addr: SocketAddr,
    sender: UdpSocketBuffer,
    receiver: UdpSocketBuffer,
}

impl Transport for UdpSocket {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn split(self) -> (BoxedSender, BoxedReceiver, Option<BoxedCloseFn>) {
        (Box::new(self.sender), Box::new(self.receiver), None)
    }
}

#[derive(Clone)]
pub struct UdpSocketBuffer {
    /// The underlying UDP Socket. This is wrapped in an Arc<Mutex<>> so that it
    /// can be shared between threads
    socket: Arc<Mutex<std::net::UdpSocket>>,
    buffer: [u8; MTU],
}

impl PacketSender for UdpSocketBuffer {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        self.socket
            .as_ref()
            .lock()
            .unwrap()
            .send_to(payload, address)?;
        Ok(())
    }
}

impl PacketReceiver for UdpSocketBuffer {
    /// Receives a packet from the socket, and stores the results in the provided buffer
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        match self
            .socket
            .as_ref()
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
    use std::net::SocketAddr;
    use std::str::FromStr;

    use anyhow::Context;
    use bevy::utils::Duration;

    use crate::transport::middleware::conditioner::{LinkConditioner, LinkConditionerConfig};
    use crate::transport::middleware::PacketReceiverWrapper;
    use crate::transport::udp::UdpSocketBuilder;
    use crate::transport::{PacketReceiver, PacketSender, Transport, TransportBuilder};

    #[test]
    fn test_udp_socket() -> Result<(), anyhow::Error> {
        // let the OS assign a port
        let local_addr = SocketAddr::from_str("127.0.0.1:0")?;
        let client_socket = UdpSocketBuilder { local_addr }
            .connect()
            .context("could not connect to socket")?;
        let client_addr = client_socket.local_addr();
        let (mut client_sender, _, _) = client_socket.split();

        let server_socket = UdpSocketBuilder { local_addr }
            .connect()
            .context("could not connect to socket")?;
        let server_addr = server_socket.local_addr();
        let (_, mut server_receiver, _) = server_socket.split();

        let msg = b"hello world";
        client_sender.send(msg, &server_addr)?;

        // sleep a little to give time to the message to arrive in the socket
        std::thread::sleep(Duration::from_millis(10));

        let Some((recv_msg, address)) = server_receiver.recv()? else {
            panic!("expected to receive a packet");
        };
        assert_eq!(address, client_addr);
        assert_eq!(recv_msg, msg);
        Ok(())
    }

    #[test]
    fn test_udp_socket_with_conditioner() -> Result<(), anyhow::Error> {
        use mock_instant::MockClock;

        // let the OS assign a port
        let local_addr = SocketAddr::from_str("127.0.0.1:0")?;

        let client_socket = UdpSocketBuilder { local_addr }
            .connect()
            .context("could not connect to socket")?;
        let client_addr = client_socket.local_addr();
        let (mut client_sender, _, _) = client_socket.split();

        let server_socket = UdpSocketBuilder { local_addr }
            .connect()
            .context("could not connect to socket")?;
        let server_addr = server_socket.local_addr();
        let (_, server_receiver, _) = server_socket.split();

        let mut conditioned_server_receiver = LinkConditioner::new(LinkConditionerConfig {
            incoming_latency: Duration::from_millis(100),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        })
        .wrap(server_receiver);

        let msg = b"hello world";
        client_sender.send(msg, &server_addr)?;

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
