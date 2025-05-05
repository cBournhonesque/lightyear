use alloc::sync::Arc;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, format, vec, vec::Vec};
use async_compat::Compat;
use bevy::platform::collections::HashMap;
use bevy::tasks::{futures_lite, IoTaskPool};
use core::future::Future;
use core::ops::Deref;
use futures_util::stream::FusedStream;
use futures_util::{future, pin_mut, stream::TryStreamExt, SinkExt, StreamExt, TryFutureExt};
use std::{
    io::BufReader,
    net::{SocketAddr, SocketAddrV4},
};
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
use tracing::{debug, error, info, trace};

use crate::client::io::transport::{ClientTransportBuilder, ClientTransportEnum};
use crate::client::io::{ClientIoEvent, ClientIoEventReceiver, ClientNetworkEventSender};
use crate::transport::error::{Error, Result};
use crate::transport::io::IoState;
use crate::transport::{
    BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport, LOCAL_SOCKET, MTU,
};

pub(crate) struct WebSocketClientSocketBuilder {
    pub(crate) server_addr: SocketAddr,
}

impl ClientTransportBuilder for WebSocketClientSocketBuilder {
    fn connect(
        self,
    ) -> Result<(
        ClientTransportEnum,
        IoState,
        Option<ClientIoEventReceiver>,
        Option<ClientNetworkEventSender>,
    )> {
        let (serverbound_tx, mut serverbound_rx) = unbounded_channel::<Message>();
        let (clientbound_tx, clientbound_rx) = unbounded_channel::<Message>();
        let (close_tx, close_rx) = async_channel::bounded(1);
        // channels used to check the status of the io task
        let (status_tx, status_rx) = async_channel::bounded(1);

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
                        status_tx
                            .send(ClientIoEvent::Disconnected(e.into()))
                            .await
                            .unwrap();
                        return;
                    }
                };
                info!("WebSocket handshake has been successfully completed");
                status_tx.send(ClientIoEvent::Connected).await.unwrap();
                let (mut write, mut read) = ws_stream.split();

                let status_tx_clone = status_tx.clone();
                let send_handle = IoTaskPool::get().spawn(Compat::new(async move {
                    while let Some(msg) = read.next().await {
                        match msg {
                            Err(e) => {
                                error!("Error while receiving websocket msg: {}", e);
                                let _ = status_tx_clone.send(ClientIoEvent::Disconnected(e.into())).await;
                                continue;
                            },
                            Ok(msg) => {
                                clientbound_tx.send(msg).expect(
                                    "Unable to propagate the read websocket message to the receiver",
                                );
                            }
                        }
                    }
                    // when we reach this point, the stream is closed
                }));
                let status_tx_clone = status_tx.clone();
                let recv_handle = IoTaskPool::get().spawn(Compat::new(async move {
                    while let Some(msg) = serverbound_rx.recv().await {
                        if let Err(e) = write.send(msg).await {
                            error!("Encountered error while sending websocket msg: {}", e);
                            let _ = status_tx_clone.send(ClientIoEvent::Disconnected(e.into())).await;
                            continue;
                        }
                    }
                }));
                // wait for a signal that the io should be closed
                let _ = close_rx.recv().await;
                let _ = status_tx
                    .send(ClientIoEvent::Disconnected(
                        std::io::Error::other("websocket closed").into(),
                    ))
                    .await;
                info!("Close websocket connection");
                send_handle.cancel().await;
                recv_handle.cancel().await;
            }))
            .detach();
        Ok((
            ClientTransportEnum::WebSocketClient(WebSocketClientSocket {
                local_addr: self.server_addr,
                sender,
                receiver,
            }),
            IoState::Connecting,
            Some(ClientIoEventReceiver(status_rx)),
            Some(ClientNetworkEventSender(close_tx)),
        ))
    }
}

pub struct WebSocketClientSocket {
    local_addr: SocketAddr,
    sender: WebSocketClientSocketSender,
    receiver: WebSocketClientSocketReceiver,
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

    fn split(self) -> (BoxedSender, BoxedReceiver) {
        (Box::new(self.sender), Box::new(self.receiver))
    }
}

struct WebSocketClientSocketSender {
    serverbound_tx: UnboundedSender<Message>,
}

impl PacketSender for WebSocketClientSocketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        self.serverbound_tx
            .send(Message::Binary(payload.to_vec().into()))
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
