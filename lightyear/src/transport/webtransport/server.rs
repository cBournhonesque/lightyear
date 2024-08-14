//! WebTransport client implementation.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_compat::Compat;
use bevy::tasks::IoTaskPool;
use bevy::utils::HashMap;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace};
use wtransport;
use wtransport::datagram::Datagram;
use wtransport::Connection;
use wtransport::{Identity, ServerConfig};

use crate::server::io::transport::{ServerTransportBuilder, ServerTransportEnum};
use crate::server::io::{ServerIoEvent, ServerIoEventReceiver, ServerNetworkEventSender};
use crate::transport::error::Result;
use crate::transport::io::IoState;
use crate::transport::{
    BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport, MIN_MTU, MTU,
};

pub(crate) struct WebTransportServerSocketBuilder {
    pub(crate) server_addr: SocketAddr,
    pub(crate) certificate: Identity,
}

impl ServerTransportBuilder for WebTransportServerSocketBuilder {
    fn start(
        self,
    ) -> Result<(
        ServerTransportEnum,
        IoState,
        Option<ServerIoEventReceiver>,
        Option<ServerNetworkEventSender>,
    )> {
        let (to_client_sender, to_client_receiver) =
            mpsc::unbounded_channel::<(Box<[u8]>, SocketAddr)>();
        let (from_client_sender, from_client_receiver) = mpsc::unbounded_channel();
        // channels used to cancel the task
        let (close_tx, close_rx) = async_channel::unbounded();
        // channels used to check the status of the io task
        let (status_tx, status_rx) = async_channel::unbounded();
        let to_client_senders = Arc::new(Mutex::new(HashMap::new()));
        let addr_to_task = Arc::new(Mutex::new(HashMap::new()));

        let sender = WebTransportServerSocketSender {
            server_addr: self.server_addr,
            to_client_senders: to_client_senders.clone(),
        };
        let receiver = WebTransportServerSocketReceiver {
            buffer: [0; MTU],
            server_addr: self.server_addr,
            from_client_receiver,
        };

        let mut config = ServerConfig::builder()
            .with_bind_address(self.server_addr)
            .with_identity(&self.certificate)
            .build();
        let mut quic_config = wtransport::quinn::TransportConfig::default();
        quic_config
            .initial_mtu(MIN_MTU as u16)
            .min_mtu(MIN_MTU as u16);
        config
            .quic_config_mut()
            .transport_config(Arc::new(quic_config));
        // need to run this with Compat because it requires the tokio reactor
        IoTaskPool::get()
            .spawn(Compat::new(async move {
                let endpoint = match wtransport::Endpoint::server(config) {
                    Ok(e) => e,
                    Err(e) => {
                        status_tx
                            .send(ServerIoEvent::ServerDisconnected(e.into()))
                            .await
                            .unwrap();
                        return;
                    }
                };
                info!("Starting server webtransport task");
                status_tx.send(ServerIoEvent::ServerConnected).await.unwrap();
                loop {
                    tokio::select! {
                        // event from netcode
                        Ok(event) = close_rx.recv() => {
                            match event {
                                ServerIoEvent::ServerDisconnected(e) => {
                                    debug!("Stopping webtransport io task. Reason: {:?}", e);
                                    drop(addr_to_task);
                                    return;
                                }
                                ServerIoEvent::ClientDisconnected(addr) => {
                                    debug!("Stopping webtransport io task associated with address: {:?} because we received a disconnection signal from netcode", addr);
                                    addr_to_task.lock().unwrap().remove(&addr);
                                }
                                _ => {}
                            }
                        }
                        // new client connecting
                        incoming_session = endpoint.accept() => {
                            let Ok(session_request) = incoming_session
                                .await
                                .inspect_err(|e| {
                                    error!("failed to accept new client: {:?}", e);
                                }) else {
                                continue;
                            };
                            let Ok(connection) = session_request
                                .accept()
                                .await
                                .inspect_err(|e| {
                                    error!("failed to accept new client: {:?}", e);
                                }) else {
                                continue;
                            };
                            let client_addr = connection.remote_address();
                            let connection = Arc::new(connection);
                            let from_client_sender = from_client_sender.clone();
                            let to_client_senders = to_client_senders.clone();
                            let task = IoTaskPool::get()
                                .spawn(Compat::new(WebTransportServerSocket::handle_client(
                                    connection,
                                    from_client_sender,
                                    to_client_senders,
                                    status_tx.clone(),
                                )));
                            addr_to_task.lock().unwrap().insert(client_addr, task);
                        }
                    }
                }
            }))
            .detach();

        Ok((
            ServerTransportEnum::WebTransportServer(WebTransportServerSocket {
                local_addr: self.server_addr,
                sender,
                receiver,
            }),
            IoState::Connecting,
            Some(ServerIoEventReceiver(status_rx)),
            Some(ServerNetworkEventSender(close_tx)),
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
        connection: Arc<Connection>,
        from_client_sender: UnboundedSender<(Datagram, SocketAddr)>,
        to_client_channels: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<Box<[u8]>>>>>,
        status_tx: async_channel::Sender<ServerIoEvent>,
    ) {
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
        // notify netcode that the io task got disconnected
        let _ = status_tx
            .send(ServerIoEvent::ClientDisconnected(client_addr))
            .await;
        to_client_channels.lock().unwrap().remove(&client_addr);
        debug!("Dropping tasks");
        // the handles being dropped cancels the tasks
    }
}

impl Transport for WebTransportServerSocket {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn split(self) -> (BoxedSender, BoxedReceiver) {
        (Box::new(self.sender), Box::new(self.receiver))
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
