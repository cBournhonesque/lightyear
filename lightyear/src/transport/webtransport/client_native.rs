#![cfg(not(target_family = "wasm"))]
//! WebTransport client implementation.
use std::net::SocketAddr;
use std::sync::Arc;

use async_compat::Compat;
use bevy::tasks::{futures_lite, IoTaskPool, TaskPool};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, error, info, trace, warn};
use wtransport;
use wtransport::datagram::Datagram;
use wtransport::ClientConfig;

use crate::transport::error::{Error, Result};
use crate::transport::io::IoState;
use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, MTU,
};

pub(crate) struct WebTransportClientSocketBuilder {
    pub(crate) client_addr: SocketAddr,
    pub(crate) server_addr: SocketAddr,
}

impl TransportBuilder for WebTransportClientSocketBuilder {
    fn connect(self) -> Result<(TransportEnum, IoState)> {
        let (to_server_sender, mut to_server_receiver) = mpsc::unbounded_channel();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();
        // channels used to cancel the task
        let (close_tx, mut close_rx) = mpsc::channel(1);
        // channels used to check the status of the io task
        let (status_tx, status_rx) = mpsc::channel(1);

        IoTaskPool::get().spawn(Compat::new(async move {
            let config = ClientConfig::builder()
                .with_bind_address(self.client_addr)
                .with_no_cert_validation()
                .build();
            let server_url = format!("https://{}", self.server_addr);
            info!(
                "Connecting to server via webtransport at server url: {}",
                &server_url
            );
            // TODO: how to get the errors from here? via a channel?

            let endpoint = match wtransport::Endpoint::client(config) {
                Ok(e) => {e}
                Err(e) => {
                    error!("Error creating webtransport endpoint: {:?}", e);
                    status_tx.send(Some(e.into())).await.unwrap();
                    return
                }
            };
            let connection = match endpoint.connect(&server_url).await {
                Ok(c) => {c}
                Err(e) => {
                    error!("Error creating webtransport connection: {:?}", e);
                    status_tx.send(Some(e.into())).await.unwrap();
                    return
                }
            };
            // signal that the io is connected
            status_tx.send(None).await.unwrap();
            info!("Connected.");

            let connection = Arc::new(connection);

            // NOTE (IMPORTANT!):
            // - we spawn two different futures for receive and send datagrams
            // - if we spawned only one future and used tokio::select!(), the branch that is not selected would be cancelled
            // - this means that we might recreate a new future in `connection.receive_datagram()` instead of just continuing
            //   to poll the existing one. This is FAULTY behaviour
            // - if you want to use tokio::Select, you have to first pin the Future, and then select on &mut Future. Only the reference gets
            //   cancelled
            let connection_recv = connection.clone();
            let recv_handle = IoTaskPool::get().spawn(Compat::new(async move {
                loop {
                    match connection_recv.receive_datagram().await {
                        Ok(data) => {
                            trace!("receive datagram from server: {:?}", &data);
                            from_server_sender.send(data).unwrap();
                        }
                        Err(e) => {
                            error!("receive_datagram connection error: {:?}", e);
                        }
                    }
                }
            }));
            let connection_send = connection.clone();
            let send_handle = IoTaskPool::get().spawn(Compat::new(async move {
                loop {
                    if let Some(msg) = to_server_receiver.recv().await {
                        trace!("send datagram to server: {:?}", &msg);
                        connection_send.send_datagram(msg).unwrap_or_else(|e| {
                            error!("send_datagram error: {:?}", e);
                        });
                    }
                }
            }));

            // Wait for a close signal from the close channel, or for the quic connection to be closed
            tokio::select! {
                reason = connection.closed() => {info!("WebTransport connection closed. Reason: {reason:?}")},
                _ = close_rx.recv() => {info!("WebTransport connection closed. Reason: client requested disconnection.");}
            }
            // close the other tasks
            recv_handle.cancel().await;
            send_handle.cancel().await;
            }))
            .detach();

        let sender = WebTransportClientPacketSender { to_server_sender };
        let receiver = WebTransportClientPacketReceiver {
            server_addr: self.server_addr,
            from_server_receiver,
            buffer: [0; MTU],
        };
        Ok((
            TransportEnum::WebTransportClient(WebTransportClientSocket {
                local_addr: self.client_addr,
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

/// WebTransport client socket
pub struct WebTransportClientSocket {
    local_addr: SocketAddr,
    sender: WebTransportClientPacketSender,
    receiver: WebTransportClientPacketReceiver,
    close_sender: mpsc::Sender<()>,
}

impl Transport for WebTransportClientSocket {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
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

struct WebTransportClientPacketSender {
    to_server_sender: mpsc::UnboundedSender<Box<[u8]>>,
}

impl PacketSender for WebTransportClientPacketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        let data = payload.to_vec().into_boxed_slice();
        self.to_server_sender
            .send(data)
            .map_err(|e| std::io::Error::other(format!("send_datagram error: {:?}", e)).into())
    }
}

struct WebTransportClientPacketReceiver {
    server_addr: SocketAddr,
    from_server_receiver: mpsc::UnboundedReceiver<Datagram>,
    buffer: [u8; MTU],
}

impl PacketReceiver for WebTransportClientPacketReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        match self.from_server_receiver.try_recv() {
            Ok(data) => {
                // convert from datagram to payload via xwt
                self.buffer[..data.len()].copy_from_slice(data.payload().as_ref());
                Ok(Some((&mut self.buffer[..data.len()], self.server_addr)))
            }
            Err(e) => {
                if e == TryRecvError::Empty {
                    Ok(None)
                } else {
                    Err(std::io::Error::other(format!("receive_datagram error: {:?}", e)).into())
                }
            }
        }
    }
}
