#![cfg(target_family = "wasm")]
//! WebTransport client implementation.
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;

use bevy::tasks::{IoTaskPool, TaskPool};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, error, info, trace};
use xwt_core::prelude::*;

use crate::client::io::transport::{ClientTransportBuilder, ClientTransportEnum};
use crate::client::io::{ClientIoEvent, ClientIoEventReceiver, ClientNetworkEventSender};
use crate::server::io::transport::{ServerTransportBuilder, ServerTransportEnum};
use crate::server::io::{ServerIoEventReceiver, ServerNetworkEventSender};
use crate::transport::error::{Error, Result};
use crate::transport::io::IoState;
use crate::transport::{BoxedReceiver, BoxedSender, PacketReceiver, PacketSender, Transport, MTU};

// Adapted from https://github.com/briansmith/ring/blob/befdc87ac7cbca615ab5d68724f4355434d3a620/src/test.rs#L364-L393
pub fn from_hex(hex_str: &str) -> std::result::Result<Vec<u8>, String> {
    if hex_str.len() % 2 != 0 {
        return Err(String::from(
            "Hex string does not have an even number of digits",
        ));
    }

    let mut result = Vec::with_capacity(hex_str.len() / 2);
    for digits in hex_str.as_bytes().chunks(2) {
        let hi = from_hex_digit(digits[0])?;
        let lo = from_hex_digit(digits[1])?;
        result.push((hi * 0x10) | lo);
    }
    Ok(result)
}

fn from_hex_digit(d: u8) -> std::result::Result<u8, String> {
    use core::ops::RangeInclusive;
    const DECIMAL: (u8, RangeInclusive<u8>) = (0, b'0'..=b'9');
    const HEX_LOWER: (u8, RangeInclusive<u8>) = (10, b'a'..=b'f');
    const HEX_UPPER: (u8, RangeInclusive<u8>) = (10, b'A'..=b'F');
    for (offset, range) in &[DECIMAL, HEX_LOWER, HEX_UPPER] {
        if range.contains(&d) {
            return Ok(d - range.start() + offset);
        }
    }
    Err(format!("Invalid hex digit '{}'", d as char))
}

pub struct WebTransportClientSocketBuilder {
    pub(crate) client_addr: SocketAddr,
    pub(crate) server_addr: SocketAddr,
    pub(crate) certificate_digest: String,
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
        // TODO: This can exhaust all available memory unless there is some other way to limit the amount of in-flight data in place
        let (to_server_sender, mut to_server_receiver) = mpsc::unbounded_channel();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();
        // channels used to cancel the task
        let (close_tx, close_rx) = async_channel::bounded(1);
        // channels used to check the status of the io task
        let (status_tx, status_rx) = async_channel::bounded(1);

        let server_url = format!("https://{}", self.server_addr);
        info!(
            "Starting client webtransport task with server url: {}",
            &server_url
        );

        let options = xwt_web::WebTransportOptions {
            server_certificate_hashes: vec![xwt_web::CertificateHash {
                algorithm: xwt_web::HashAlgorithm::Sha256,
                value: from_hex(&self.certificate_digest).unwrap(),
            }],
            ..Default::default()
        };
        let endpoint = xwt_web::Endpoint {
            options: options.to_js(),
        };

        let (send, recv) = tokio::sync::oneshot::channel();
        let (send2, recv2) = tokio::sync::oneshot::channel();
        let (send3, recv3) = tokio::sync::oneshot::channel();
        let status_tx_clone = status_tx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            info!("Starting webtransport io thread");

            let connecting = match endpoint.connect(&server_url).await {
                Ok(e) => e,
                Err(e) => {
                    error!("Error creating webtransport connection: {:?}", e);
                    status_tx_clone
                        .send(ClientIoEvent::Disconnected(
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
                    status_tx_clone
                        .send(ClientIoEvent::Disconnected(
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
            status_tx_clone
                .send(ClientIoEvent::Connected)
                .await
                .unwrap();
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
        wasm_bindgen_futures::spawn_local(async move {
            let Ok(connection) = recv.await else {
                return;
            };
            loop {
                tokio::select! {
                    _ = connection.transport.closed() => return,
                    to_send = connection.receive_datagram() => {
                        match to_send {
                           Ok(data) => {
                               trace!("receive datagram from server: {:?}", &data);
                               from_server_sender.send(data).unwrap();
                           }
                           Err(e) => {
                               error!("receive_datagram connection error: {:?}", e);
                           }
                        }
                    }
                }
            }
        });
        wasm_bindgen_futures::spawn_local(async move {
            let Ok(connection) = recv2.await else {
                return;
            };
            loop {
                tokio::select! {
                    _ = connection.transport.closed() => return,
                    recv = to_server_receiver.recv() => {
                        if let Some(msg) = recv {
                            trace!("send datagram to server: {:?}", &msg);
                            connection.send_datagram(msg).await.unwrap_or_else(|e| {
                                error!("send_datagram error: {:?}", e);
                            });
                        }
                    }
                }
            }
        });
        wasm_bindgen_futures::spawn_local(async move {
            let Ok(connection) = recv3.await else {
                return;
            };
            // Wait for a close signal from the close channel, or for the quic connection to be closed
            tokio::select! {
                reason = connection.transport.closed() => {
                    info!("WebTransport connection closed. Reason: {reason:?}");
                    status_tx.send(ClientIoEvent::Disconnected(std::io::Error::other(format!("Error: {:?}", reason)).into())).await.unwrap();
                },
                _ = close_rx.recv() => {
                    connection.transport.close();
                    info!("WebTransport connection closed. Reason: client requested disconnection.");
                }
            }
        });

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
            Some(ClientIoEventReceiver(status_rx)),
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
