use std::ops::Deref;
use std::{
    future::Future,
    io::BufReader,
    net::{SocketAddr, SocketAddrV4},
    sync::Arc,
};

use anyhow::Result;
use async_compat::Compat;
use bevy::tasks::IoTaskPool;
use bevy::utils::hashbrown::HashMap;

use tokio::{
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, error::TryRecvError, unbounded_channel, UnboundedReceiver, UnboundedSender},
        Mutex,
    },
};

use futures_util::stream::FusedStream;
use futures_util::{
    future, pin_mut,
    stream::{SplitSink, TryStreamExt},
    SinkExt, StreamExt, TryFutureExt,
};

use tokio_tungstenite::{
    connect_async, connect_async_with_config, tungstenite::Message, MaybeTlsStream,
};
use tracing::{debug, info, trace};
use tracing_log::log::error;

use crate::transport::{PacketReceiver, PacketSender, Transport, LOCAL_SOCKET};

use super::MTU;

pub struct WebSocketClientSocket {
    server_addr: SocketAddr,
}

impl WebSocketClientSocket {
    pub(crate) fn new(server_addr: SocketAddr) -> Self {
        Self { server_addr }
    }

    /*fn get_tls_connector(&self) -> TlsConnector {
        let root_store = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        TlsConnector::from(Arc::new(config))
    }*/
}

impl Transport for WebSocketClientSocket {
    fn local_addr(&self) -> SocketAddr {
        // TODO: get the local_addr
        // match ws_stream.get_ref() {
        //     MaybeTlsStream::Plain(s) => {
        //         s.local_addr()
        //         info!("WebSocket connection is not encrypted");
        //     }
        // }
        LOCAL_SOCKET
    }

    fn listen(self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
        let (serverbound_tx, mut serverbound_rx) = unbounded_channel::<Message>();
        let (clientbound_tx, clientbound_rx) = unbounded_channel::<Message>();

        let packet_sender = WebSocketClientSocketSender { serverbound_tx };

        let packet_receiver = WebSocketClientSocketReceiver {
            buffer: [0; MTU],
            server_addr: self.server_addr,
            clientbound_rx,
        };

        IoTaskPool::get()
            .spawn(Compat::new(async move {
                info!("Starting client websocket task");
                let (ws_stream, _) =
                    connect_async_with_config(format!("ws://{}/", self.server_addr), None, true)
                        .await
                        .expect("Unable to connect to websocket server");
                info!("WebSocket handshake has been successfully completed");

                let (mut write, mut read) = ws_stream.split();

                IoTaskPool::get()
                    .spawn(async move {
                        while let Some(msg) = read.next().await {
                            let msg = msg
                                .map_err(|e| {
                                    error!("Error while receiving websocket msg: {}", e);
                                })
                                .unwrap();

                            clientbound_tx.send(msg).expect(
                                "Unable to propagate the read websocket message to the receiver",
                            );
                        }
                        // when we reach this point, the stream is closed
                    })
                    .detach();

                IoTaskPool::get()
                    .spawn(async move {
                        while let Some(msg) = serverbound_rx.recv().await {
                            write
                                .send(msg)
                                .await
                                .map_err(|e| {
                                    error!("Encountered error while sending websocket msg: {}", e);
                                })
                                .unwrap();
                        }
                    })
                    .detach();
            }))
            .detach();

        (Box::new(packet_sender), Box::new(packet_receiver))
    }
}

struct WebSocketClientSocketSender {
    serverbound_tx: UnboundedSender<Message>,
}

impl PacketSender for WebSocketClientSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> std::io::Result<()> {
        self.serverbound_tx
            .send(Message::Binary(payload.to_vec()))
            .map_err(|e| {
                std::io::Error::other(format!("unable to send message to server: {:?}", e))
            })
    }
}

struct WebSocketClientSocketReceiver {
    buffer: [u8; MTU],
    server_addr: SocketAddr,
    clientbound_rx: UnboundedReceiver<Message>,
}

impl PacketReceiver for WebSocketClientSocketReceiver {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        match self.clientbound_rx.try_recv() {
            Ok(msg) => match msg {
                Message::Binary(buf) => {
                    self.buffer[..buf.len()].copy_from_slice(&buf);
                    Ok(Some((&mut self.buffer[..buf.len()], self.server_addr)))
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
                    Err(std::io::Error::other(format!(
                        "unable to receive message from client: {}",
                        e
                    )))
                }
            }
        }
    }
}
