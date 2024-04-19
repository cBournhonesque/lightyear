#![cfg(target_family = "wasm")]
//! WebTransport client implementation.
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;

use base64::prelude::{Engine as _, BASE64_STANDARD};
use bevy::tasks::{IoTaskPool, TaskPool};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, error, info, trace};
use xwt_core::prelude::*;

use crate::transport::error::{Error, Result};
use crate::transport::io::IoState;
use crate::transport::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport,
    TransportBuilder, TransportEnum, MTU,
};

pub struct WebTransportClientSocketBuilder {
    pub(crate) client_addr: SocketAddr,
    pub(crate) server_addr: SocketAddr,
    pub(crate) certificate_digest: String,
}

impl TransportBuilder for WebTransportClientSocketBuilder {
    fn connect(self) -> Result<(TransportEnum, IoState)> {
        // TODO: This can exhaust all available memory unless there is some other way to limit the amount of in-flight data in place
        let (to_server_sender, mut to_server_receiver) = mpsc::unbounded_channel();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();
        // channels used to cancel the task
        let (close_tx, mut close_rx) = mpsc::channel(1);
        // channels used to check the status of the io task
        let (status_tx, status_rx) = mpsc::channel(1);

        let server_url = format!("https://{}", self.server_addr);
        info!(
            "Starting client webtransport task with server url: {}",
            &server_url
        );

        let options = xwt_web_sys::WebTransportOptions {
            server_certificate_hashes: vec![xwt_web_sys::CertificateHash {
                algorithm: xwt_web_sys::HashAlgorithm::Sha256,
                value: ring::test::from_hex(&self.certificate_digest).unwrap(),
            }],
            ..Default::default()
        };
        let endpoint = xwt_web_sys::Endpoint {
            options: options.to_js(),
        };

        let (send, recv) = tokio::sync::oneshot::channel();
        let (send2, recv2) = tokio::sync::oneshot::channel();
        let (send3, recv3) = tokio::sync::oneshot::channel();
        IoTaskPool::get().spawn_local(async move {
            info!("Starting webtransport io thread");

            let connecting = match endpoint.connect(&server_url).await {
                Ok(e) => e,
                Err(e) => {
                    error!("Error creating webtransport connection: {:?}", e);
                    status_tx
                        .send(Some(
                            std::io::Error::other("error creating webtransport connection").into(),
                        ))
                        .await
                        .unwrap();
                    return;
                }
            };
            let connection = match connecting.wait_connect().await {
                Ok(c) => c,
                Err(e) => {
                    error!("Error connecting to server: {:?}", e);
                    status_tx
                        .send(Some(
                            std::io::Error::other(
                                "error connecting webtransport endpoint to server",
                            )
                            .into(),
                        ))
                        .await
                        .unwrap();
                    return;
                }
            };
            // signal that the io is connected
            status_tx.send(None).await.unwrap();
            let connection = Rc::new(connection);
            send.send(connection.clone()).unwrap();
            send2.send(connection.clone()).unwrap();
            send3.send(connection.clone()).unwrap();
        });

        // NOTE (IMPORTANT!):
        // - we spawn two different futures for receive and send datagrams
        // - if we spawned only one future and used tokio::select!(), the branch that is not selected would be cancelled
        // - this means that we might recreate a new future in `connection.receive_datagram()` instead of just continuing
        //   to poll the existing one. This is FAULTY behaviour
        // - if you want to use tokio::Select, you have to first pin the Future, and then select on &mut Future. Only the reference gets
        //   cancelled
        IoTaskPool::get()
            .spawn(async move {
                let Ok(connection) = recv.await else {
                    return;
                };
                loop {
                    match connection.receive_datagram().await {
                        Ok(data) => {
                            trace!("receive datagram from server: {:?}", &data);
                            from_server_sender.send(data).unwrap();
                        }
                        Err(e) => {
                            error!("receive_datagram connection error: {:?}", e);
                        }
                    }
                }
            })
            .detach();
        IoTaskPool::get()
            .spawn(async move {
                let Ok(connection) = recv2.await else {
                    return;
                };
                loop {
                    if let Some(msg) = to_server_receiver.recv().await {
                        trace!("send datagram to server: {:?}", &msg);
                        connection.send_datagram(msg).await.unwrap_or_else(|e| {
                            error!("send_datagram error: {:?}", e);
                        });
                    }
                }
            })
            .detach();
        IoTaskPool::get()
            .spawn(async move {
                let Ok(connection) = recv3.await else {
                    return;
                };
                // Wait for a close signal from the close channel, or for the quic connection to be closed
                close_rx.recv().await;
                info!("WebTransport connection closed.");
                // close the connection
                connection.transport.close();
                // TODO: how do we close the other tasks?
            })
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
    from_server_receiver: mpsc::UnboundedReceiver<Vec<u8>>,
    buffer: [u8; MTU],
}

impl PacketReceiver for WebTransportClientPacketReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        match self.from_server_receiver.try_recv() {
            Ok(datagram) => {
                // convert from datagram to payload via xwt
                let data = datagram.as_slice();
                self.buffer[..data.len()].copy_from_slice(data);
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
