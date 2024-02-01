use std::{io::BufReader, net::SocketAddr, sync::Arc};

use anyhow::Result;
use bevy::utils::hashbrown::HashMap;
use fastwebsockets::{
    upgrade::{upgrade, UpgradeFut},
    FragmentCollector, Frame, OpCode, Payload,
};
use http_body_util::Empty;
use hyper::{
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
    Request, Response,
};
use hyper_util::rt::TokioIo;
use rustls_pemfile::{certs, rsa_private_keys};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, error::TryRecvError, UnboundedReceiver, UnboundedSender},
        Mutex,
    },
};
use tokio_rustls::{rustls::ServerConfig, TlsAcceptor};
use tracing::{info, trace};
use tracing_log::log::error;

use crate::transport::{PacketReceiver, PacketSender, Transport};

use super::MTU;

pub struct WebSocketServerSocketTLSConfig {
    pub keys: Box<[u8]>,
    pub certs: Box<[u8]>,
}

pub struct WebSocketServerSocket {
    server_addr: SocketAddr,
    tls_config: Option<WebSocketServerSocketTLSConfig>,
}

pub enum WebSocketMessage {
    Binary(Vec<u8>),
    Close(Option<u16>, Option<String>),
}

impl WebSocketServerSocket {
    pub(crate) fn new(
        server_addr: SocketAddr,
        tls_config: Option<WebSocketServerSocketTLSConfig>,
    ) -> Self {
        Self {
            server_addr,
            tls_config,
        }
    }

    pub async fn handle_client(
        fut: UpgradeFut,
        client_addr: SocketAddr,
        from_client_sender: UnboundedSender<(WebSocketMessage, SocketAddr)>,
        to_client_channels: Arc<
            std::sync::Mutex<HashMap<SocketAddr, UnboundedSender<WebSocketMessage>>>,
        >,
    ) {
        let mut ws = fut.await.unwrap();
        ws.set_writev(false); // TODO understand this
        let ws = Arc::new(Mutex::new(FragmentCollector::new(ws)));

        let (to_client_sender, mut to_client_receiver) =
            mpsc::unbounded_channel::<WebSocketMessage>();

        let abort_sender = to_client_sender.clone();

        to_client_channels
            .lock()
            .unwrap()
            .insert(client_addr, to_client_sender);

        let recv = ws.clone();
        tokio::spawn(async move {
            loop {
                // receive messages from client
                match recv.lock().await.read_frame().await {
                    Ok(frame) => match frame.opcode {
                        OpCode::Close => {
                            trace!("received close frame from client!");
                            from_client_sender
                                .send((WebSocketMessage::Close(None, None), client_addr))
                                .unwrap();
                            to_client_channels.lock().unwrap().remove(&client_addr);
                            break;
                        }
                        OpCode::Binary => {
                            trace!("received packet from client!: {:?}", &frame.payload);
                            from_client_sender
                                .send((WebSocketMessage::Binary(frame.payload.into()), client_addr))
                                .unwrap();
                        }
                        _ => {}
                    },
                    Err(e) => {
                        error!("WebSocket read_frame error: {:?}", e);
                    }
                }
            }
        });

        let send = ws.clone();
        tokio::spawn(async move {
            loop {
                if let Some(msg) = to_client_receiver.recv().await {
                    match msg {
                        WebSocketMessage::Close(code, reason) => {
                            trace!("sending close frame to client!");
                            // codes: https://www.rfc-editor.org/rfc/rfc6455.html#section-7.1.5
                            send.lock()
                                .await
                                .write_frame(Frame::close(
                                    code.unwrap_or(1000),
                                    reason.unwrap_or("".to_string()).as_bytes(),
                                ))
                                .await
                                .map_err(|e| error!("WebSocket send close frame error: {:?}", e))
                                .unwrap();
                            break;
                        }
                        WebSocketMessage::Binary(data) => {
                            trace!("sending packet to client!: {:?}", &data);
                            send.lock()
                                .await
                                .write_frame(Frame::binary(Payload::Owned(data)))
                                .await
                                .map_err(|e| error!("WebSocket send binary frame error: {:?}", e))
                                .unwrap();
                        }
                    }
                }
            }
        });
    }

    fn get_tls_acceptor(&self) -> Option<TlsAcceptor> {
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
    }
}

impl Transport for WebSocketServerSocket {
    fn local_addr(&self) -> SocketAddr {
        self.server_addr
    }

    fn listen(self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
        let (to_client_sender, to_client_receiver) =
            mpsc::unbounded_channel::<(WebSocketMessage, SocketAddr)>();
        let (from_client_sender, from_client_receiver) = mpsc::unbounded_channel();
        let to_client_senders = Arc::new(std::sync::Mutex::new(HashMap::new()));

        let packet_sender = WebSocketServerSocketSender {
            server_addr: self.server_addr,
            to_client_senders: to_client_senders.clone(),
        };

        let packet_receiver = WebSocketServerSocketReceiver {
            buffer: [0; MTU],
            server_addr: self.server_addr,
            from_client_receiver,
        };

        tokio::spawn(async move {
            info!("Starting server websocket task");
            let acceptor = self.get_tls_acceptor();
            let listener = TcpListener::bind(self.server_addr).await.unwrap();
            loop {
                let to_client_senders = to_client_senders.clone();
                let from_client_sender = from_client_sender.clone();
                let (stream, client_addr) = listener.accept().await.unwrap();
                info!("got client");
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let to_client_senders = to_client_senders.clone();
                    let from_client_sender = from_client_sender.clone();
                    if let Some(acceptor) = acceptor {
                        let stream = acceptor.accept(stream).await.unwrap();
                        let io = TokioIo::new(stream);
                        let conn_fut = http1::Builder::new()
                            .serve_connection(
                                io,
                                service_fn(move |mut req: Request<Incoming>| {
                                    let to_client_senders = to_client_senders.clone();
                                    let from_client_sender = from_client_sender.clone();
                                    async move {
                                        let (response, fut) = upgrade(&mut req)?;
                                        tokio::spawn(Self::handle_client(
                                            fut,
                                            client_addr,
                                            from_client_sender,
                                            to_client_senders,
                                        ));
                                        anyhow::Ok(response)
                                    }
                                })
                            )
                            .with_upgrades()
                            .await;
                    }
                });
            }
        });

        (Box::new(packet_sender), Box::new(packet_receiver))
    }
}

struct WebSocketServerSocketSender {
    server_addr: SocketAddr,
    to_client_senders:
        Arc<std::sync::Mutex<HashMap<SocketAddr, UnboundedSender<WebSocketMessage>>>>,
}

impl PacketSender for WebSocketServerSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> std::io::Result<()> {
        if let Some(to_client_sender) = self.to_client_senders.lock().unwrap().get(address) {
            to_client_sender
                .send(WebSocketMessage::Binary(payload.into()))
                .map_err(|e| {
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

struct WebSocketServerSocketReceiver {
    buffer: [u8; MTU],
    server_addr: SocketAddr,
    from_client_receiver: UnboundedReceiver<(WebSocketMessage, SocketAddr)>,
}

impl PacketReceiver for WebSocketServerSocketReceiver {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        match self.from_client_receiver.try_recv() {
            Ok((data, addr)) => match data {
                WebSocketMessage::Binary(buf) => {
                    self.buffer[..buf.len()].copy_from_slice(&buf);
                    Ok(Some((&mut self.buffer[..buf.len()], self.server_addr)))
                }
                _ => Ok(None),
            },
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
