#![cfg(not(target_family = "wasm"))]
//! WebTransport client implementation.
use std::net::SocketAddr;
use std::sync::Arc;

use async_compat::Compat;
use bevy::tasks::IoTaskPool;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, error, info, trace};
use wtransport;
use wtransport::datagram::Datagram;
use wtransport::error::ConnectingError;
use wtransport::ClientConfig;

use crate::client::io::transport::{ClientTransportBuilder, ClientTransportEnum};
use crate::client::io::{ClientIoEvent, ClientIoEventReceiver, ClientNetworkEventSender};
use crate::transport::error::{Error, Result};
use crate::transport::io::IoState;
use crate::transport::{
    BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport, MIN_MTU, MTU,
};

pub(crate) struct WebTransportClientSocketBuilder {
    pub(crate) client_addr: SocketAddr,
    pub(crate) server_addr: SocketAddr,
}

impl ClientTransportBuilder for WebTransportClientSocketBuilder {
    fn connect(
        self,
    ) -> Result<(
        ClientTransportEnum,
        IoState,
        Option<ClientIoEventReceiver>,
        Option<ClientNetworkEventSender>,
    )> {
        let (to_server_sender, mut to_server_receiver) = mpsc::unbounded_channel::<Box<[u8]>>();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();
        // channels used to cancel the task
        let (close_tx, close_rx) = async_channel::bounded(1);
        // channels used to check the status of the io task
        let (event_tx, event_rx) = async_channel::bounded(1);

        IoTaskPool::get().spawn(Compat::new(async move {
            let mut config = ClientConfig::builder()
                .with_bind_address(self.client_addr)
                .with_no_cert_validation()
                .build();
            let mut quic_config = wtransport::quinn::TransportConfig::default();
            quic_config
                .initial_mtu(MIN_MTU as u16)
                .min_mtu(MIN_MTU as u16);
            config.quic_config_mut().transport_config(
                Arc::new(quic_config)
            );
            let server_url = format!("https://{}", self.server_addr);
            info!(
                "Connecting to server via webtransport at server url: {}",
                &server_url
            );
            // TODO: we should listen to the close channel in parallel here
            let endpoint = match wtransport::Endpoint::client(config) {
                Ok(e) => {e}
                Err(e) => {
                    error!("Error creating webtransport endpoint: {:?}", e);
                    let _ = event_tx.send(ClientIoEvent::Disconnected(e.into())).await;
                    return
                }
            };

            tokio::select! {
                _ = close_rx.recv() => {
                    info!("WebTransport connection closed. Reason: client requested disconnection.");
                    let _ = event_tx.send(ClientIoEvent::Disconnected(std::io::Error::other("received close signal").into())).await;
                    return
                }
                connection = endpoint.connect(&server_url) => {
                    let connection = match connection {
                        Ok(c) => {c}
                        Err(e) => {
                            error!("Error creating webtransport connection: {:?}", e);
                            let _ = event_tx.send(ClientIoEvent::Disconnected(std::io::Error::other(e).into())).await;
                            return
                        }
                    };
                    // signal that the io is connected
                    event_tx.send(ClientIoEvent::Connected).await.unwrap();
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
                                    // all the ConnectionErrors are related to the connection being close, so we can close the task
                                    error!("receive_datagram connection error: {:?}", e);
                                    return;
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
                                    error!("send_datagram via webtransport error: {:?}", e);
                                });
                            }
                        }
                    }));
                    // Wait for a close signal from the close channel, or for the quic connection to be closed
                    tokio::select! {
                        reason = connection.closed() => {
                            info!("WebTransport connection closed. Reason: {reason:?}. Shutting down webtransport tasks.");
                            event_tx.send(ClientIoEvent::Disconnected(Error::WebTransport(ConnectingError::ConnectionError(reason)))).await.unwrap();
                        },
                        _ = close_rx.recv() => {
                            info!("WebTransport connection closed. Reason: client requested disconnection. Shutting down webtransport tasks.");
                        }
                    }
                    // close the other tasks

                    // NOTE: for some reason calling `cancel()` doesn't work (the task still keeps running indefinitely)
                    //  instead we just drop the task handle
                    // drop(recv_handle);
                    // drop(send_handle);
                    recv_handle.cancel().await;
                    send_handle.cancel().await;
                    debug!("WebTransport tasks shut down.");
                }
            }
            }))
            .detach();

        let sender = WebTransportClientPacketSender { to_server_sender };
        let receiver = WebTransportClientPacketReceiver {
            server_addr: self.server_addr,
            from_server_receiver,
            buffer: [0; MTU],
        };
        Ok((
            ClientTransportEnum::WebTransportClient(WebTransportClientSocket {
                local_addr: self.client_addr,
                sender,
                receiver,
            }),
            IoState::Connecting,
            Some(ClientIoEventReceiver(event_rx)),
            Some(ClientNetworkEventSender(close_tx)),
        ))
    }
}

/// WebTransport client socket
pub struct WebTransportClientSocket {
    local_addr: SocketAddr,
    sender: WebTransportClientPacketSender,
    receiver: WebTransportClientPacketReceiver,
}

impl Transport for WebTransportClientSocket {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn split(self) -> (BoxedSender, BoxedReceiver) {
        (Box::new(self.sender), Box::new(self.receiver))
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
