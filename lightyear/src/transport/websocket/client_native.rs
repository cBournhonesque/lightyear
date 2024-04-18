use std::ops::Deref;
use std::{
    future::Future,
    io::BufReader,
    net::{SocketAddr, SocketAddrV4},
    sync::Arc,
};

use async_compat::Compat;
use bevy::tasks::{futures_lite, IoTaskPool};
use bevy::utils::hashbrown::HashMap;
use futures_util::stream::FusedStream;
use futures_util::{future, pin_mut, stream::TryStreamExt, SinkExt, StreamExt, TryFutureExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{
            self, error::TryRecvError, unbounded_channel, Sender, UnboundedReceiver,
            UnboundedSender,
        },
        Mutex,
    },
};
use tokio_tungstenite::{
    connect_async, connect_async_with_config, tungstenite::Message, MaybeTlsStream,
};
use tracing::{debug, info, trace};
use tracing_log::log::error;

use crate::transport::error::{Error, Result};
use crate::transport::io::IoState;
use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, LOCAL_SOCKET, MTU,
};

pub(crate) struct WebSocketClientSocketBuilder {
    pub(crate) server_addr: SocketAddr,
}

impl TransportBuilder for WebSocketClientSocketBuilder {
    fn connect(self) -> Result<(TransportEnum, IoState)> {
        let (serverbound_tx, mut serverbound_rx) = unbounded_channel::<Message>();
        let (clientbound_tx, clientbound_rx) = unbounded_channel::<Message>();
        let (close_tx, mut close_rx) = mpsc::channel(1);
        // channels used to check the status of the io task
        let (status_tx, status_rx) = mpsc::channel(1);

        let sender = WebSocketClientSocketSender { serverbound_tx };
        let receiver = WebSocketClientSocketReceiver {
            buffer: [0; MTU],
            server_addr: self.server_addr,
            clientbound_rx,
        };

        IoTaskPool::get()
            .spawn(Compat::new(async move {
                let ws_stream = match connect_async_with_config(
                    format!("ws://{}/", self.server_addr),
                    None,
                    true,
                )
                .await
                {
                    Ok((ws_stream, _)) => ws_stream,
                    Err(e) => {
                        status_tx.send(Some(e.into())).await.unwrap();
                        return;
                    }
                };
                info!("WebSocket handshake has been successfully completed");
                let (mut write, mut read) = ws_stream.split();

                let send_handle = IoTaskPool::get().spawn(Compat::new(async move {
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
                }));
                let recv_handle = IoTaskPool::get().spawn(Compat::new(async move {
                    while let Some(msg) = serverbound_rx.recv().await {
                        write
                            .send(msg)
                            .await
                            .map_err(|e| {
                                error!("Encountered error while sending websocket msg: {}", e);
                            })
                            .unwrap();
                    }
                }));
                // wait for a signal that the io should be closed
                close_rx.recv().await;
                info!("Close websocket connection");
                send_handle.cancel().await;
                recv_handle.cancel().await;
            }))
            .detach();
        Ok((
            TransportEnum::WebSocketClient(WebSocketClientSocket {
                local_addr: self.server_addr,
                sender,
                receiver,
                close_sender: close_tx,
            }),
            IoState::Connecting {
                error_channel: status_rx,
            },
        ))
    }
}

pub struct WebSocketClientSocket {
    local_addr: SocketAddr,
    sender: WebSocketClientSocketSender,
    receiver: WebSocketClientSocketReceiver,
    close_sender: mpsc::Sender<()>,
}

impl WebSocketClientSocket {
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

    fn split(self) -> (BoxedSender, BoxedReceiver, Option<BoxedCloseFn>) {
        let close_fn = move || {
            self.close_sender
                .blocking_send(())
                .map_err(|e| Error::from(std::io::Error::other(format!("close error: {:?}", e))))
        };
        (
            Box::new(self.sender),
            Box::new(self.receiver),
            Some(Box::new(close_fn)),
        )
    }
}

struct WebSocketClientSocketSender {
    serverbound_tx: UnboundedSender<Message>,
}

impl PacketSender for WebSocketClientSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        self.serverbound_tx
            .send(Message::Binary(payload.to_vec()))
            .map_err(|e| {
                Error::WebSocket(
                    std::io::Error::other(format!("unable to send message to server: {:?}", e))
                        .into(),
                )
            })
    }
}

struct WebSocketClientSocketReceiver {
    buffer: [u8; MTU],
    server_addr: SocketAddr,
    clientbound_rx: UnboundedReceiver<Message>,
}

impl PacketReceiver for WebSocketClientSocketReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
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
