//! WebTransport client implementation.
use anyhow::Context;
use async_compat::Compat;
use bevy::tasks::{futures_lite, IoTaskPool};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace};
use wtransport;
use wtransport::datagram::Datagram;
use wtransport::endpoint::endpoint_side::Server;
use wtransport::endpoint::IncomingSession;
use wtransport::tls::Certificate;
use wtransport::ServerConfig;
use wtransport::{Connection, Endpoint};

use crate::transport::error::{Error, Result};
use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, MTU,
};

pub(crate) struct WebTransportServerSocketBuilder {
    pub(crate) server_addr: SocketAddr,
    pub(crate) certificate: Certificate,
}

impl TransportBuilder for WebTransportServerSocketBuilder {
    fn connect(self) -> Result<TransportEnum> {
        let (to_client_sender, to_client_receiver) =
            mpsc::unbounded_channel::<(Box<[u8]>, SocketAddr)>();
        let (from_client_sender, from_client_receiver) = mpsc::unbounded_channel();
        let to_client_senders = Arc::new(Mutex::new(HashMap::new()));

        let config = ServerConfig::builder()
            .with_bind_address(self.server_addr)
            .with_certificate(self.certificate)
            .build();
        // need to run this with Compat because it requires the tokio reactor
        let endpoint = futures_lite::future::block_on(Compat::new(async {
            let endpoint = wtransport::Endpoint::server(config)?;
            Ok::<_, Error>(endpoint)
        }))?;

        let sender = WebTransportServerSocketSender {
            server_addr: self.server_addr,
            to_client_senders: to_client_senders.clone(),
        };
        let receiver = WebTransportServerSocketReceiver {
            buffer: [0; MTU],
            server_addr: self.server_addr,
            from_client_receiver,
        };

        IoTaskPool::get()
            .spawn(Compat::new(async move {
                info!("Starting server webtransport task");
                loop {
                    // clone the channel for each client
                    let from_client_sender = from_client_sender.clone();
                    let to_client_senders = to_client_senders.clone();

                    // new client connecting
                    let incoming_session = endpoint.accept().await;

                    IoTaskPool::get()
                        .spawn(Compat::new(WebTransportServerSocket::handle_client(
                            incoming_session,
                            from_client_sender,
                            to_client_senders,
                        )))
                        .detach();
                }
            }))
            .detach();

        Ok(TransportEnum::WebTransportServer(
            WebTransportServerSocket {
                local_addr: self.server_addr,
                sender,
                receiver,
            },
        ))
    }
}

/// WebTransport client socket
pub struct WebTransportServerSocket {
    local_addr: SocketAddr,
    sender: WebTransportServerSocketSender,
    receiver: WebTransportServerSocketReceiver,
}

impl WebTransportServerSocket {
    pub async fn handle_client(
        incoming_session: IncomingSession,
        from_client_sender: UnboundedSender<(Datagram, SocketAddr)>,
        to_client_channels: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<Box<[u8]>>>>>,
    ) {
        let session_request = incoming_session
            .await
            .map_err(|e| {
                error!("failed to accept new client: {:?}", e);
            })
            .unwrap();

        let connection = session_request
            .accept()
            .await
            .map_err(|e| {
                error!("failed to accept new client: {:?}", e);
            })
            .unwrap();
        let connection = Arc::new(connection);
        let client_addr = connection.remote_address();

        info!(
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
        let connection_recv = connection.clone();
        let from_client_handle = IoTaskPool::get().spawn(async move {
            loop {
                // receive messages from client
                match connection_recv.receive_datagram().await {
                    Ok(data) => {
                        trace!(
                            "received datagram from client!: {:?} {:?}",
                            data.as_ref(),
                            data.len()
                        );
                        from_client_sender.send((data, client_addr)).unwrap();
                    }
                    Err(e) => {
                        error!("receive_datagram connection error: {:?}", e);
                        // to_client_channels.lock().unwrap().remove(&client_addr);
                        break;
                    }
                }
            }
        });
        let connection_send = connection.clone();
        let to_client_handle = IoTaskPool::get().spawn(async move {
            loop {
                if let Some(msg) = to_client_receiver.recv().await {
                    trace!("sending datagram to client!: {:?}", &msg);
                    connection_send
                        .send_datagram(msg.as_ref())
                        .unwrap_or_else(|e| {
                            error!("send_datagram error: {:?}", e);
                        });
                }
            }
        });

        // await for the quic connection to be closed for any reason
        let reason = connection.closed().await;
        info!(
            "Connection with {} closed. Reason: {:?}",
            client_addr, reason
        );
        to_client_channels.lock().unwrap().remove(&client_addr);
        debug!("Dropping tasks");
        // the handles being dropped cancels the tasks
        // TODO: need to disconnect the client in netcode
    }
}

impl Transport for WebTransportServerSocket {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn split(self) -> (BoxedSender, BoxedReceiver, Option<BoxedCloseFn>) {
        (Box::new(self.sender), Box::new(self.receiver), None)
    }
}

struct WebTransportServerSocketSender {
    server_addr: SocketAddr,
    to_client_senders: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<Box<[u8]>>>>>,
}

impl PacketSender for WebTransportServerSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        if let Some(to_client_sender) = self.to_client_senders.lock().unwrap().get(address) {
            to_client_sender.send(payload.into()).map_err(|e| {
                std::io::Error::other(format!("unable to send message to client: {}", e)).into()
            })
        } else {
            // consider that if the channel doesn't exist, it's because the connection was closed
            Ok(())
            // Err(std::io::Error::other(format!(
            //     "unable to find channel for client: {}",
            //     address
            // )))
        }
    }
}

struct WebTransportServerSocketReceiver {
    buffer: [u8; MTU],
    server_addr: SocketAddr,
    from_client_receiver: UnboundedReceiver<(Datagram, SocketAddr)>,
}
impl PacketReceiver for WebTransportServerSocketReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
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
                    ))
                    .into())
                }
            }
        }
    }
}
