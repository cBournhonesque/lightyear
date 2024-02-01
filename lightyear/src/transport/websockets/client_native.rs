use std::{future::Future, io::BufReader, net::SocketAddr, sync::Arc};

use anyhow::Result;
use bevy::utils::hashbrown::HashMap;
use fastwebsockets::{
    handshake::{client, generate_key},
    upgrade::{upgrade, UpgradeFut},
    FragmentCollector, Frame, OpCode, Payload, WebSocket,
};
use http_body_util::Empty;
use hyper::{
    body::{Bytes, Incoming},
    header::{CONNECTION, HOST, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE},
    server::conn::http1,
    service::service_fn,
    upgrade::Upgraded,
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
use tokio_rustls::{
    rustls::{pki_types::ServerName, ClientConfig, RootCertStore, ServerConfig},
    TlsAcceptor, TlsConnector,
};
use tracing::{info, trace};
use tracing_log::log::error;

use crate::transport::{PacketReceiver, PacketSender, Transport};

use super::MTU;

pub struct WebSocketClientSocketTLSConfig {
    pub server_name: String,
}

pub struct WebSocketClientSocket {
    client_addr: SocketAddr,
    server_addr: SocketAddr,
    tls_config: Option<WebSocketClientSocketTLSConfig>,
}

pub enum WebSocketMessage {
    Binary(Vec<u8>),
    Close(Option<u16>, Option<String>),
}

struct SpawnExecutor;

impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
where
    Fut: Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    fn execute(&self, fut: Fut) {
        tokio::task::spawn(fut);
    }
}

impl WebSocketClientSocket {
    pub(crate) fn new(
        client_addr: SocketAddr,
        server_addr: SocketAddr,
        tls_config: Option<WebSocketClientSocketTLSConfig>,
    ) -> Self {
        Self {
            client_addr,
            server_addr,
            tls_config,
        }
    }

    pub async fn handle_connection(
        mut ws: WebSocket<TokioIo<Upgraded>>,
        from_server_sender: UnboundedSender<WebSocketMessage>,
        mut to_server_receiver: UnboundedReceiver<WebSocketMessage>,
    ) {
        ws.set_writev(false);

        let ws = Arc::new(Mutex::new(FragmentCollector::new(ws)));

        let recv = ws.clone();
        tokio::spawn(async move {
            loop {
                // receive messages from server
                match recv.lock().await.read_frame().await {
                    Ok(frame) => match frame.opcode {
                        OpCode::Close => {
                            trace!("received close frame from server!");
                            // TODO parse code & reason
                            from_server_sender
                                .send(WebSocketMessage::Close(None, None))
                                .unwrap();
                            break;
                        }
                        OpCode::Binary => {
                            trace!("received packet from server!: {:?}", &frame.payload);
                            from_server_sender
                                .send(WebSocketMessage::Binary(frame.payload.into()))
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
                if let Some(msg) = to_server_receiver.recv().await {
                    match msg {
                        WebSocketMessage::Close(code, reason) => {
                            trace!("sending close frame to server!");
                            // codes: https://www.rfc-editor.org/rfc/rfc6455.html#section-7.1.5
                            send.lock()
                                .await
                                .write_frame(Frame::close(
                                    code.unwrap_or(1000),
                                    reason.unwrap_or("".to_string()).as_bytes(),
                                ))
                                .await;
                            break;
                        }
                        WebSocketMessage::Binary(data) => {
                            trace!("sending packet to server!: {:?}", &data);
                            send.lock()
                                .await
                                .write_frame(Frame::binary(Payload::Owned(data)))
                                .await;
                        }
                    }
                }
            }
        });
    }

    fn get_tls_connector(&self) -> TlsConnector {
        let root_store = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        TlsConnector::from(Arc::new(config))
    }
}

impl Transport for WebSocketClientSocket {
    fn local_addr(&self) -> SocketAddr {
        self.server_addr
    }

    fn listen(self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
        let (to_server_sender, to_server_receiver) = mpsc::unbounded_channel();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();

        let packet_sender = WebSocketClientSocketSender { to_server_sender };

        let packet_receiver = WebSocketClientSocketReceiver {
            buffer: [0; MTU],
            server_addr: self.server_addr,
            from_server_receiver,
        };

        tokio::spawn(async move {
            info!("Starting server websocket task");

            let tcp_stream = TcpStream::connect(self.server_addr).await.unwrap();

            let (protocol, host) = if let Some(tls_config) = &self.tls_config {
                ("wss", tls_config.server_name)
            } else {
                (
                    "ws",
                    format!("{}:{}", self.server_addr.ip(), self.server_addr.port()),
                )
            };

            let req = Request::builder()
                .method("GET")
                .uri(format!(
                    "{}://{}:{}/",
                    protocol,
                    self.server_addr.ip(),
                    self.server_addr.port()
                ))
                .header(HOST, host)
                .header(UPGRADE, "websocket")
                .header(CONNECTION, "upgrade")
                .header(SEC_WEBSOCKET_KEY, generate_key())
                .header(SEC_WEBSOCKET_VERSION, "13")
                .body(Empty::<Bytes>::new())
                .unwrap();

            let ws = if let Some(tls_config) = &self.tls_config {
                let tls_stream = self
                    .get_tls_connector()
                    .connect(
                        ServerName::try_from(tls_config.server_name).unwrap(),
                        tcp_stream,
                    )
                    .await
                    .unwrap();
                client(&SpawnExecutor, req, tls_stream).await.unwrap().0
            } else {
                client(&SpawnExecutor, req, tcp_stream).await.unwrap().0
            };

            Self::handle_connection(ws, from_server_sender, to_server_receiver);
        });

        (Box::new(packet_sender), Box::new(packet_receiver))
    }
}

struct WebSocketClientSocketSender {
    to_server_sender: mpsc::UnboundedSender<WebSocketMessage>,
}

impl PacketSender for WebSocketClientSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> std::io::Result<()> {
        self.to_server_sender
            .send(WebSocketMessage::Binary(payload.into()))
            .map_err(|e| {
                std::io::Error::other(format!("unable to send message to server: {:?}", e))
            })
    }
}

struct WebSocketClientSocketReceiver {
    buffer: [u8; MTU],
    server_addr: SocketAddr,
    from_server_receiver: UnboundedReceiver<WebSocketMessage>,
}

impl PacketReceiver for WebSocketClientSocketReceiver {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        match self.from_server_receiver.try_recv() {
            Ok(data) => match data {
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
