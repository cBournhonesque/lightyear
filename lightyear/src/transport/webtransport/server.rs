use crate::transport::webtransport::MTU;
use crate::transport::{PacketReceiver, PacketSender, Transport};
use bevy::tasks::{IoTaskPool, TaskPool};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info};
use wtransport;
use wtransport::datagram::Datagram;
use wtransport::endpoint::IncomingSession;
use wtransport::tls::Certificate;
use wtransport::{ClientConfig, ServerConfig};

pub struct WebTransportServerSocket {
    server_addr: SocketAddr,
    certificate: Option<Certificate>,
}

impl WebTransportServerSocket {
    pub(crate) fn new(server_addr: SocketAddr, certificate: Certificate) -> Self {
        Self {
            server_addr,
            certificate: Some(certificate),
        }
    }

    pub async fn handle_client(
        incoming_session: IncomingSession,
        from_client_sender: UnboundedSender<(Datagram, SocketAddr)>,
        to_client_channels: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<Box<[u8]>>>>>,
    ) {
        let session_request = incoming_session.await.unwrap();
        let connection = session_request.accept().await.unwrap();
        let client_addr = connection.remote_address();
        debug!(
            "Spawning new task to create connection with client: {}",
            client_addr
        );

        // add a new pair of channels for this client
        let (to_client_sender, mut to_client_receiver) = mpsc::unbounded_channel::<Box<[u8]>>();
        to_client_channels
            .lock()
            .unwrap()
            .insert(client_addr, to_client_sender);

        // connection established, waiting for data from client
        loop {
            tokio::select! {
                // receive messages from client
                x = connection.receive_datagram() => {
                    match x {
                        Ok(data) => {
                            from_client_sender.send((data, client_addr)).unwrap();
                        }
                        Err(e) => {
                            error!("receive_datagram error: {:?}", e);
                        }
                    }
                }
                // send messages to client
                Some(msg) = to_client_receiver.recv() => {
                    connection.send_datagram(msg.as_ref()).unwrap_or_else(|e| {
                        error!("send_datagram error: {:?}", e);
                    });
                }
            }
        }
    }
}

impl Transport for WebTransportServerSocket {
    fn local_addr(&self) -> SocketAddr {
        self.server_addr
    }

    fn listen(&mut self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
        // TODO: should i create my own task pool?
        let server_addr = self.server_addr;
        let certificate = std::mem::take(&mut self.certificate).unwrap();
        let (to_client_sender, to_client_receiver) =
            mpsc::unbounded_channel::<(Box<[u8]>, SocketAddr)>();
        let (from_client_sender, from_client_receiver) = mpsc::unbounded_channel();
        let to_client_senders = Arc::new(Mutex::new(HashMap::new()));

        let packet_sender = WebTransportServerSocketSender {
            server_addr,
            to_client_senders: to_client_senders.clone(),
        };
        let packet_receiver = WebTransportServerSocketReceiver {
            buffer: [0; MTU],
            server_addr,
            from_client_receiver,
        };

        tokio::spawn(async move {
            debug!("Starting server webtransport task");
            let config = ServerConfig::builder()
                .with_bind_address(server_addr)
                .with_certificate(certificate)
                .build();
            let server = wtransport::Endpoint::server(config).unwrap();

            loop {
                // clone the channel for each client
                let from_client_sender = from_client_sender.clone();
                let to_client_senders = to_client_senders.clone();

                // new client connecting
                let incoming_session = server.accept().await;

                tokio::spawn(Self::handle_client(
                    incoming_session,
                    from_client_sender,
                    to_client_senders,
                ));
            }
        });
        (Box::new(packet_sender), Box::new(packet_receiver))
    }
}

pub struct WebTransportServerSocketSender {
    server_addr: SocketAddr,
    to_client_senders: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<Box<[u8]>>>>>,
}

impl PacketSender for WebTransportServerSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> std::io::Result<()> {
        if let Some(to_client_sender) = self.to_client_senders.lock().unwrap().get(address) {
            to_client_sender.send(payload.into()).map_err(|e| {
                std::io::Error::other(format!("unable to send message to client: {}", e))
            })
        } else {
            Err(std::io::Error::other(format!(
                "unable to find channel for client: {}",
                address
            )))
        }
    }
}

pub struct WebTransportServerSocketReceiver {
    buffer: [u8; MTU],
    server_addr: SocketAddr,
    from_client_receiver: UnboundedReceiver<(Datagram, SocketAddr)>,
}
impl PacketReceiver for WebTransportServerSocketReceiver {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        match self.from_client_receiver.try_recv() {
            Ok((data, addr)) => {
                self.buffer[..data.len()].copy_from_slice(data.payload().as_ref());
                Ok(Some((&mut self.buffer[..data.len()], addr)))
            }
            Err(e) => {
                if e == TryRecvError::Empty {
                    Ok(None)
                } else {
                    Err(std::io::Error::other(format!(
                        "unable to receive message from client: {}",
                        e
                    )))
                }
            }
        }
    }
}
