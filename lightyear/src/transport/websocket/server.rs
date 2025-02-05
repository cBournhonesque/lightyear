use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use async_compat::Compat;
use bevy::tasks::{futures_lite, IoTaskPool};
use bevy::utils::HashMap;
use futures_util::{
    future, pin_mut,
    stream::{SplitSink, TryStreamExt},
    SinkExt, StreamExt, TryFutureExt,
};
use tokio::sync::mpsc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver, UnboundedSender},
};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tracing::{debug, info, trace};
use tracing_log::log::error;

use crate::server::io::transport::{ServerTransportBuilder, ServerTransportEnum};
use crate::server::io::{ServerIoEvent, ServerIoEventReceiver, ServerNetworkEventSender};
use crate::transport::error::{Error, Result};
use crate::transport::io::IoState;
use crate::transport::{BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport, MTU};

pub(crate) struct WebSocketServerSocketBuilder {
    pub(crate) server_addr: SocketAddr,
}

impl ServerTransportBuilder for WebSocketServerSocketBuilder {
    fn start(
        self,
    ) -> Result<(
        ServerTransportEnum,
        IoState,
        Option<ServerIoEventReceiver>,
        Option<ServerNetworkEventSender>,
    )> {
        let (serverbound_tx, serverbound_rx) = unbounded_channel::<(SocketAddr, Message)>();
        let clientbound_tx_map = ClientBoundTxMap::new(Mutex::new(HashMap::new()));
        // channels used to cancel the task
        let (close_tx, close_rx) = async_channel::unbounded();
        // channels used to check the status of the io task
        let (status_tx, status_rx) = async_channel::unbounded();
        let addr_to_task = Arc::new(Mutex::new(HashMap::new()));

        let sender = WebSocketServerSocketSender {
            server_addr: self.server_addr,
            addr_to_clientbound_tx: clientbound_tx_map.clone(),
        };
        let receiver = WebSocketServerSocketReceiver {
            buffer: [0; MTU],
            server_addr: self.server_addr,
            serverbound_rx,
        };

        IoTaskPool::get()
            .spawn(Compat::new(async move {
                let listener = match TcpListener::bind(self.server_addr).await {
                    Ok(l) => l,
                    Err(e) => {
                        status_tx
                            .send(ServerIoEvent::ServerDisconnected(e.into()))
                            .await
                            .unwrap();
                        return;
                    }
                };
                info!("Starting server websocket task");
                status_tx
                    .send(ServerIoEvent::ServerConnected)
                    .await
                    .unwrap();

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
                                    clientbound_tx_map.lock().unwrap().remove(&addr);
                                }
                                _ => {}
                            }
                        }
                        Ok((stream, addr)) = listener.accept() => {
                            let clientbound_tx_map = clientbound_tx_map.clone();
                            let serverbound_tx = serverbound_tx.clone();
                            let task = IoTaskPool::get().spawn(Compat::new(
                                WebSocketServerSocket::handle_client(addr, stream, serverbound_tx, clientbound_tx_map, status_tx.clone())
                            ));
                            addr_to_task.lock().unwrap().insert(addr, task);
                        }
                    }
                }
            }))
            .detach();
        Ok((
            ServerTransportEnum::WebSocketServer(WebSocketServerSocket {
                local_addr: self.server_addr,
                sender,
                receiver,
            }),
            IoState::Connecting,
            Some(ServerIoEventReceiver(status_rx)),
            None,
        ))
    }
}

pub struct WebSocketServerSocket {
    local_addr: SocketAddr,
    sender: WebSocketServerSocketSender,
    receiver: WebSocketServerSocketReceiver,
}

impl WebSocketServerSocket {
    /*fn get_tls_acceptor(&self) -> Option<TlsAcceptor> {
        if let Some(config) = &self.tls_config {
            let server_config = ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    certs(&mut BufReader::new(&*config.certs))
                        .map(|e| e.unwrap())
                        .collect(),
                    rsa_private_keys(&mut BufReader::new(&*config.keys))
                        .map(|e| e.unwrap().into())
                        .next()
                        .unwrap(),
                )
                .unwrap();
            Some(TlsAcceptor::from(Arc::new(server_config)))
        } else {
            None
        }
    }*/
}

impl WebSocketServerSocket {
    async fn handle_client(
        addr: SocketAddr,
        stream: TcpStream,
        serverbound_tx: UnboundedSender<(SocketAddr, Message)>,
        clientbound_tx_map: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<Message>>>>,
        status_tx: async_channel::Sender<ServerIoEvent>,
    ) {
        let Ok(ws_stream) = tokio_tungstenite::accept_async(stream)
            .await
            .inspect_err(|e| error!("An error occured during the websocket handshake: {e:?}"))
        else {
            return;
        };
        info!("New WebSocket connection: {}", addr);

        let (clientbound_tx, mut clientbound_rx) = unbounded_channel::<Message>();
        let (mut write, mut read) = ws_stream.split();
        clientbound_tx_map
            .lock()
            .unwrap()
            .insert(addr, clientbound_tx);

        let clientbound_handle = IoTaskPool::get().spawn(async move {
            while let Some(msg) = clientbound_rx.recv().await {
                write
                    .send(msg)
                    .await
                    .map_err(|e| {
                        error!("Encountered error while sending websocket msg: {}", e);
                    })
                    .unwrap();
            }
            write.close().await.unwrap_or_else(|e| {
                error!("Error closing websocket: {:?}", e);
            });
        });
        let serverbound_handle = IoTaskPool::get().spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(msg) => {
                        serverbound_tx
                            .send((addr, msg))
                            .unwrap_or_else(|e| error!("receive websocket error: {:?}", e));
                    }
                    Err(e) => {
                        error!("receive websocket error: {:?}", e);
                    }
                }
            }
        });

        let _closed = futures_lite::future::race(clientbound_handle, serverbound_handle).await;

        info!("Connection with {} closed", addr);
        clientbound_tx_map.lock().unwrap().remove(&addr);
        // notify netcode that the io task got disconnected
        let _ = status_tx
            .send(ServerIoEvent::ClientDisconnected(addr))
            .await;
        // dropping the task handles cancels them
    }
}

type ClientBoundTxMap = Arc<Mutex<HashMap<SocketAddr, UnboundedSender<Message>>>>;

impl Transport for WebSocketServerSocket {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn split(self) -> (BoxedSender, BoxedReceiver) {
        (Box::new(self.sender), Box::new(self.receiver))
    }
}

struct WebSocketServerSocketSender {
    server_addr: SocketAddr,
    addr_to_clientbound_tx: ClientBoundTxMap,
}

impl PacketSender for WebSocketServerSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        if let Some(clientbound_tx) = self.addr_to_clientbound_tx.lock().unwrap().get(address) {
            clientbound_tx
                .send(Message::Binary(payload.to_vec()))
                .map_err(|e| {
                    Error::WebSocket(
                        std::io::Error::other(format!("unable to send message to client: {}", e))
                            .into(),
                    )
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

struct WebSocketServerSocketReceiver {
    buffer: [u8; MTU],
    server_addr: SocketAddr,
    serverbound_rx: UnboundedReceiver<(SocketAddr, Message)>,
}

impl PacketReceiver for WebSocketServerSocketReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        match self.serverbound_rx.try_recv() {
            Ok((addr, msg)) => match msg {
                Message::Binary(buf) => {
                    self.buffer[..buf.len()].copy_from_slice(&buf);
                    Ok(Some((&mut self.buffer[..buf.len()], addr)))
                }
                Message::Close(frame) => {
                    info!("WebSocket connection closed (Frame: {:?})", frame);
                    Ok(None)
                }
                _ => Ok(None),
            },
            Err(e) => {
                if e == TryRecvError::Empty {
                    Ok(None)
                } else {
                    Err(Error::WebSocket(
                        std::io::Error::other(format!(
                            "unable to receive message from client: {}",
                            e
                        ))
                        .into(),
                    ))
                }
            }
        }
    }
}
